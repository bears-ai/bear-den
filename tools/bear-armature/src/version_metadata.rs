use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::{collections::BTreeMap, ffi::OsString, io::Write};

#[derive(Serialize)]
struct VersionMetadata {
    name: &'static str,
    version: &'static str,
    build_git_sha: &'static str,
    built_at_utc: &'static str,
    local_head_sha: &'static str,
    supports_session_list: bool,
    supports_session_resume: bool,
    supports_session_load: bool,
    direct_tools: Option<BTreeMap<&'static str, bool>>,
    chrome_tools: ChromeToolsStatus,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ChromeToolsStatus {
    NotProbed,
}

impl VersionMetadata {
    fn for_binary() -> Self {
        Self {
            name: "bear-armature",
            version: crate::adapter_version(),
            build_git_sha: env!("DEN_ACP_ADAPTER_GIT_SHA"),
            built_at_utc: env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
            // Introspection describes the binary, not the caller's working tree or browser.
            local_head_sha: "unavailable",
            supports_session_list: true,
            supports_session_resume: true,
            supports_session_load: true,
            direct_tools: None,
            chrome_tools: ChromeToolsStatus::NotProbed,
        }
    }
}

enum OutputFormat {
    Human,
    Json,
}

impl OutputFormat {
    fn from_args(mut args: impl Iterator<Item = OsString>) -> Result<Self> {
        match args.next() {
            None => Ok(Self::Human),
            Some(arg) if arg == "--json" && args.next().is_none() => Ok(Self::Json),
            _ => bail!("unsupported version arguments; usage: bear-armature version [--json]"),
        }
    }
}

pub(crate) fn run(args: impl Iterator<Item = OsString>) -> Result<()> {
    match OutputFormat::from_args(args)? {
        OutputFormat::Human => crate::print_version_to_stderr(),
        OutputFormat::Json => {
            let mut stdout = std::io::stdout().lock();
            serde_json::to_writer(&mut stdout, &VersionMetadata::for_binary())
                .context("write version metadata JSON")?;
            writeln!(stdout).context("write version metadata newline")?;
        }
    }
    Ok(())
}
