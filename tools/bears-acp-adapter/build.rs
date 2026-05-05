use std::{path::Path, process::Command};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=BEARS_ACP_ADAPTER_BUILD_SHA");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let built_at = build_time_utc_rfc3339();
    println!("cargo:rustc-env=BEARS_ACP_ADAPTER_BUILT_AT_UTC={built_at}");

    let build_sha = std::env::var("BEARS_ACP_ADAPTER_BUILD_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("GITHUB_SHA")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(local_repo_head_sha)
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=BEARS_ACP_ADAPTER_GIT_SHA={build_sha}");
}

fn build_time_utc_rfc3339() -> String {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(secs) = epoch.trim().parse::<i64>() {
            if let Ok(dt) = OffsetDateTime::from_unix_timestamp(secs) {
                return dt.format(&Rfc3339).expect("RFC3339 format");
            }
        }
    }

    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 format")
}

fn local_repo_head_sha() -> Option<String> {
    let repo_root = Path::new("../..");
    if !repo_root.join(".git").exists() || !repo_root.join("tools/bears-acp-adapter").is_dir() {
        return None;
    }

    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| {
            let top = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let adapter_path = Path::new(&top).join("tools/bears-acp-adapter");
            if adapter_path.is_dir() {
                Some(())
            } else {
                None
            }
        })?;

    Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .current_dir(repo_root)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}
