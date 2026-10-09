use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, process::Output, process::Stdio, time::Duration};

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct AdapterVersionInfo {
    name: String,
    version: String,
    build_git_sha: String,
    built_at_utc: String,
    local_head_sha: String,
    supports_session_list: bool,
    supports_session_resume: bool,
    supports_session_load: bool,
    direct_tools: Option<BTreeMap<String, Value>>,
    chrome_tools: String,
}

async fn cli(args: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_bear-armature"));
    command
        .args(args)
        .env_clear()
        .env("PATH", "")
        .envs(environment.iter().copied())
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::null())
        .kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .expect("CLI must exit without waiting for ACP stdin or remote services")
        .expect("run bear-armature binary")
}

fn json_metadata(output: &Output) -> AdapterVersionInfo {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let stdout = std::str::from_utf8(&output.stdout).expect("UTF-8 stdout");
    assert_eq!(stdout.lines().count(), 1, "exactly one compact JSON object");
    assert!(stdout.ends_with('\n'));
    // Parsing the entire stream rejects banners, protocol frames, and trailing objects.
    let object: BTreeMap<String, Value> = serde_json::from_str(stdout).expect("JSON object");
    assert_eq!(object.len(), 10);
    assert_eq!(object.get("direct_tools"), Some(&Value::Null));
    let info: AdapterVersionInfo = serde_json::from_str(stdout).expect("Swift-compatible schema");
    assert_eq!(info.name, "bear-armature");
    assert_eq!(info.version, env!("DEN_ACP_ADAPTER_VERSION"));
    assert_eq!(info.build_git_sha, env!("DEN_ACP_ADAPTER_GIT_SHA"));
    assert_eq!(info.built_at_utc, env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"));
    assert!(!info.version.is_empty());
    assert!(info.built_at_utc.contains('T'));
    assert!(info.built_at_utc.ends_with('Z') || info.built_at_utc.ends_with("+00:00"));
    assert_eq!(info.local_head_sha, "unavailable");
    assert!(info.supports_session_list);
    assert!(info.supports_session_resume);
    assert!(info.supports_session_load);
    assert_eq!(info.direct_tools, None);
    assert_eq!(info.chrome_tools, "not_probed");
    info
}

#[tokio::test]
async fn json_version_works_without_any_den_configuration() {
    json_metadata(&cli(&["version", "--json"], &[]).await);
}

#[tokio::test]
async fn json_version_bypasses_logging_initialization() {
    for filter in ["trace", "["] {
        json_metadata(
            &cli(
                &["version", "--json"],
                &[
                    ("RUST_LOG", "trace"),
                    ("BEARS_ARMATURE_TRACE", "1"),
                    ("BEARS_ARMATURE_TRACE_FILTER", filter),
                ],
            )
            .await,
        );
    }
}

#[tokio::test]
async fn json_version_ignores_runtime_configuration_and_secrets() {
    let baseline = json_metadata(&cli(&["version", "--json"], &[]).await);
    let output = cli(
        &["version", "--json"],
        &[
            ("DEN_API_URL", "invalid-url-with-secret"),
            ("BEAR_SLUG", "private-bear"),
            ("DEN_TOKEN", "private-token"),
            ("DEN_TOKEN_ENV", "PRIVATE_TOKEN"),
            ("PRIVATE_TOKEN", "private-indirect-token"),
            ("DEN_HOST_BROWSER_MCP_TOKEN", "private-browser-token"),
            ("BEARS_CHROME_CDP_URL", "http://private:secret@127.0.0.1:1"),
            (
                "DEN_ACP_ADAPTER_VERSION",
                "runtime-version-must-not-be-used",
            ),
            ("DEN_ACP_ADAPTER_GIT_SHA", "runtime-sha-must-not-be-used"),
            (
                "DEN_ACP_ADAPTER_BUILT_AT_UTC",
                "runtime-time-must-not-be-used",
            ),
        ],
    )
    .await;
    assert_eq!(json_metadata(&output), baseline);
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private"));
}

#[tokio::test]
async fn human_version_formats_keep_stderr_and_build_metadata() {
    for args in [
        &["version"][..],
        &["--version"][..],
        &["-V"][..],
        &["acp", "--version"][..],
        &["acp", "-V"][..],
    ] {
        let output = cli(args, &[]).await;
        assert!(output.status.success(), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
        assert!(stderr.starts_with(&format!(
            "bear-armature {}\nBuild git SHA: {}\nBuilt at UTC: {}\n",
            env!("DEN_ACP_ADAPTER_VERSION"),
            env!("DEN_ACP_ADAPTER_GIT_SHA"),
            env!("DEN_ACP_ADAPTER_BUILT_AT_UTC"),
        )));
        for field in [
            "Local HEAD SHA: ",
            "ACP sessions: list/resume/load; conversations bound via Den\n",
            "Direct tools: ",
            "Chrome tools: ",
        ] {
            assert!(stderr.contains(field), "{args:?}: missing {field}");
        }
        assert!(!stderr.contains("starting version="));
        assert!(!stderr.contains("configuration is incomplete"));
        assert!(!stderr.contains("jsonrpc"));
    }
}

#[tokio::test]
async fn unsupported_version_arguments_fail_without_entering_acp() {
    for args in [
        &["version", "--unknown"][..],
        &["version", "--json", "extra"][..],
        &["version", "--json", "--json"][..],
        &["version", "extra", "--json"][..],
        &["version", "--token", "private-token"][..],
        &["version", "--help"][..],
    ] {
        let output = cli(
            args,
            &[
                ("RUST_LOG", "trace"),
                ("BEARS_ARMATURE_TRACE", "1"),
                ("BEARS_ARMATURE_TRACE_FILTER", "["),
            ],
        )
        .await;
        assert_eq!(output.status.code(), Some(1), "{args:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert_eq!(
            String::from_utf8(output.stderr).expect("UTF-8 stderr"),
            "bear-armature: unsupported version arguments; usage: bear-armature version [--json]\n",
        );
    }
    let output = cli(&["--version", "--json"], &[]).await;
    assert!(
        !output.status.success(),
        "no new global JSON flag combination"
    );
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn help_advertises_the_version_subcommand() {
    let output = cli(&["--help"], &[]).await;
    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 stderr");
    assert!(stderr.contains("version                Show version metadata"));
    assert!(stderr.contains("bear-armature version [--json]"));
}
