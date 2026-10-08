//! ZIP parsing with independent compressed, entry-count and decompression bounds.
use super::{
    portable_hats, portable_models, BearBundleManifest, BEAR_BUNDLE_FORMAT,
    BEAR_BUNDLE_MAX_UPLOAD_BYTES, BEAR_BUNDLE_VERSION,
};
use crate::errors::CustomError;
use std::io::{Cursor, Read, Write};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

pub(super) fn build_bear_bundle(
    manifest_yaml: &str,
    memory_sqlite: &[u8],
) -> Result<Vec<u8>, CustomError> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer
        .start_file("bear.yaml", options)
        .map_err(|err| CustomError::System(format!("start bear.yaml in bundle failed: {err}")))?;
    writer
        .write_all(manifest_yaml.as_bytes())
        .map_err(|err| CustomError::System(format!("write bear.yaml to bundle failed: {err}")))?;
    writer.start_file("memory.sqlite", options).map_err(|err| {
        CustomError::System(format!("start memory.sqlite in bundle failed: {err}"))
    })?;
    writer.write_all(memory_sqlite).map_err(|err| {
        CustomError::System(format!("write memory.sqlite to bundle failed: {err}"))
    })?;
    let cursor = writer
        .finish()
        .map_err(|err| CustomError::System(format!("finish Bear bundle failed: {err}")))?;
    Ok(cursor.into_inner())
}

fn bear_bundle_entry_name(entries: &[String], basename: &str) -> Result<String, CustomError> {
    let candidates = entries
        .iter()
        .filter(|name| {
            !name.ends_with('/')
                && !name.starts_with("__MACOSX/")
                && !name.split('/').any(|part| part == "..")
                && name.rsplit('/').next() == Some(basename)
        })
        .cloned()
        .collect::<Vec<_>>();

    match candidates.as_slice() {
        [single] => Ok(single.clone()),
        [] => {
            let sample = entries
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            Err(CustomError::ValidationError(format!(
                ".bear bundle missing {basename}; entries include: {sample}"
            )))
        }
        _ => Err(CustomError::ValidationError(format!(
            ".bear bundle contains multiple {basename} entries: {}",
            candidates.join(", ")
        ))),
    }
}

pub(super) fn read_bear_bundle(bytes: &[u8]) -> Result<(BearBundleManifest, Vec<u8>), CustomError> {
    if bytes.len() > BEAR_BUNDLE_MAX_UPLOAD_BYTES {
        return Err(CustomError::ValidationError(
            ".bear bundle exceeds the 256 MiB upload limit".into(),
        ));
    }
    let mut archive = open_archive(Cursor::new(bytes))?;
    let (manifest, memory_name) = manifest_from_archive(&mut archive)?;
    let memory = read_entry(&mut archive, &memory_name, BEAR_BUNDLE_MAX_UPLOAD_BYTES)?;
    if memory.is_empty() {
        return Err(CustomError::ValidationError(
            "memory.sqlite is empty".into(),
        ));
    }
    Ok((manifest, memory))
}

/// Preview validates manifest and SQLite entry metadata, without decompressing
/// SQLite or claiming that its content/integrity has been inspected.
pub(super) fn preview_bear_bundle<R: Read + std::io::Seek>(
    reader: R,
) -> Result<BearBundleManifest, CustomError> {
    let mut archive = open_archive(reader)?;
    Ok(manifest_from_archive(&mut archive)?.0)
}

fn open_archive<R: Read + std::io::Seek>(reader: R) -> Result<ZipArchive<R>, CustomError> {
    let archive = ZipArchive::new(reader)
        .map_err(|error| CustomError::ValidationError(format!("Invalid .bear ZIP: {error}")))?;
    if archive.len() > 128 {
        return Err(CustomError::ValidationError(
            ".bear bundle contains too many ZIP entries".into(),
        ));
    }
    Ok(archive)
}

fn manifest_from_archive<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<(BearBundleManifest, String), CustomError> {
    let entries = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .map(|file| file.name().to_owned())
                .map_err(|error| {
                    CustomError::ValidationError(format!("Read .bear ZIP entry: {error}"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let manifest_name = bear_bundle_entry_name(&entries, "bear.yaml")?;
    let memory_name = bear_bundle_entry_name(&entries, "memory.sqlite")?;
    let memory = archive
        .by_name(&memory_name)
        .map_err(|_| CustomError::ValidationError("memory.sqlite is unavailable".into()))?;
    if memory.size() == 0 || memory.size() > BEAR_BUNDLE_MAX_UPLOAD_BYTES as u64 {
        return Err(CustomError::ValidationError(
            "memory.sqlite is empty or exceeds its decompressed size limit".into(),
        ));
    }
    drop(memory);
    let manifest_bytes = read_entry(archive, &manifest_name, 1024 * 1024)?;
    let manifest: BearBundleManifest = serde_yml::from_slice(&manifest_bytes)
        .map_err(|error| CustomError::ValidationError(format!("Parse bear.yaml: {error}")))?;
    if manifest.format != BEAR_BUNDLE_FORMAT
        || !(1..=BEAR_BUNDLE_VERSION).contains(&manifest.version)
    {
        return Err(CustomError::ValidationError(format!(
            "unsupported .bear format {} version {}",
            manifest.format, manifest.version
        )));
    }
    portable_hats::validate(&manifest.hats, manifest.ide_default_hat)?;
    portable_models::validate(
        manifest.model_configurations.as_deref(),
        manifest.default_model_configuration_id,
        &manifest.hats,
    )?;
    den_service::skills::validate_portable(&manifest.skills)?;
    Ok((manifest, memory_name))
}

fn read_entry<R: Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, CustomError> {
    let file = archive
        .by_name(name)
        .map_err(|error| CustomError::ValidationError(format!("Open {name}: {error}")))?;
    if file.size() > limit as u64 {
        return Err(CustomError::ValidationError(format!(
            "{name} exceeds its decompressed size limit ({limit} bytes)"
        )));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| CustomError::ValidationError(format!("Read {name}: {error}")))?;
    if bytes.len() > limit {
        return Err(CustomError::ValidationError(format!(
            "{name} exceeds its decompressed size limit ({limit} bytes)"
        )));
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "tests/bundle_io.rs"]
mod tests;
