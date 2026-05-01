use std::{path::Path, process::Command};

fn main() {
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-env-changed=BEARS_ACP_ADAPTER_BUILD_SHA");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/main.rs");
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");

    let sha = std::env::var("BEARS_ACP_ADAPTER_BUILD_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("GITHUB_SHA")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(local_repo_head_sha)
        .unwrap_or_else(|| "unknown".to_string());

    println!("cargo:rustc-env=BEARS_ACP_ADAPTER_GIT_SHA={sha}");
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
