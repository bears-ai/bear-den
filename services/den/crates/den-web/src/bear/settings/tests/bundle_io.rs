use super::*;

#[test]
fn manifest_and_sqlite_have_independent_decompression_limits() {
    let manifest = b"small manifest";
    let memory = vec![b'a'; 4096];
    let bytes = build_bear_bundle(std::str::from_utf8(manifest).unwrap(), &memory).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(read_entry(&mut archive, "bear.yaml", 32).unwrap(), manifest);
    assert!(read_entry(&mut archive, "memory.sqlite", 1024).is_err());
    assert_eq!(
        read_entry(&mut archive, "memory.sqlite", 4096).unwrap(),
        memory
    );
    assert!(read_entry(&mut archive, "bear.yaml", 4).is_err());
}

#[test]
fn wrapped_legacy_bundle_names_work_but_ambiguous_manifests_do_not() {
    let wrapped = vec![
        "Old Bear/bear.yaml".into(),
        "Old Bear/memory.sqlite".into(),
        "__MACOSX/Old Bear/bear.yaml".into(),
    ];
    assert_eq!(
        bear_bundle_entry_name(&wrapped, "bear.yaml").unwrap(),
        "Old Bear/bear.yaml"
    );
    assert_eq!(
        bear_bundle_entry_name(&wrapped, "memory.sqlite").unwrap(),
        "Old Bear/memory.sqlite"
    );
    assert!(bear_bundle_entry_name(&["../bear.yaml".into()], "bear.yaml").is_err());
    assert!(bear_bundle_entry_name(
        &["bear.yaml".into(), "another/bear.yaml".into()],
        "bear.yaml"
    )
    .is_err());
}

fn corrupt_entry_crc(bytes: &mut [u8], name: &str) {
    // ZIP discovery may read compressed payload bytes while scanning for the
    // central directory. Corrupt the recorded CRC instead: only an actual entry
    // read/decompression can reject it, without forbidding legitimate metadata IO.
    let header = (0..bytes.len().saturating_sub(46))
        .find(|&index| {
            bytes[index..index + 4] == [0x50, 0x4b, 0x01, 0x02]
                && usize::from(u16::from_le_bytes([bytes[index + 28], bytes[index + 29]]))
                    == name.len()
                && bytes.get(index + 46..index + 46 + name.len()) == Some(name.as_bytes())
        })
        .expect("central directory entry");
    let offset = header + 16;
    let original = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    bytes[offset..offset + 4].copy_from_slice(&(original ^ 1).to_le_bytes());
}

fn preview_fixture() -> Vec<u8> {
    build_bear_bundle("format: bear\nversion: 1\nbear:\n  slug: preview\n  name: Preview\n  description: Purpose\n  birthdate: '2020-01-01'\nprompts:\n  system_prompt: Steering\n", &vec![b'a'; 4 * 1024 * 1024]).unwrap()
}

#[test]
fn manifest_preview_does_not_inflate_sqlite_or_claim_its_crc_was_validated() {
    let mut bytes = preview_fixture();
    corrupt_entry_crc(&mut bytes, "memory.sqlite");
    let preview = preview_bear_bundle(Cursor::new(bytes.as_slice())).unwrap();
    assert_eq!(preview.bear.name, "Preview");
    assert!(
        matches!(read_bear_bundle(&bytes), Err(CustomError::ValidationError(message))
        if message.contains("Read memory.sqlite"))
    );
}

#[test]
fn manifest_preview_still_verifies_the_manifest_crc() {
    let mut bytes = preview_fixture();
    corrupt_entry_crc(&mut bytes, "bear.yaml");
    assert!(
        matches!(preview_bear_bundle(Cursor::new(bytes.as_slice())), Err(CustomError::ValidationError(message))
        if message.contains("Read bear.yaml"))
    );
}
