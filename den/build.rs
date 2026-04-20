use std::env;
use std::path::Path;
use std::time::Instant;

/// Env-based SHA first (Docker/CI often lack `.git` in the build context).
/// Common CI variables are fallbacks when `GIT_COMMIT` is not passed explicitly.
fn resolve_git_commit() -> String {
    for key in [
        "GIT_COMMIT",
        "SOURCE_COMMIT",
        "GITHUB_SHA",
        "CI_COMMIT_SHA",
        "CIRCLE_SHA1",
        "BUILDKITE_COMMIT",
    ] {
        if let Ok(v) = env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
    }
    if let Some(sha) = git_rev_parse_head() {
        return sha;
    }
    "unknown".to_string()
}

fn git_rev_parse_head() -> Option<String> {
    let manifest = env::var_os("CARGO_MANIFEST_DIR")?;
    let out = std::process::Command::new("git")
        .current_dir(manifest)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn emit_git_commit_rerun_hints(manifest_dir: &str) {
    for key in [
        "GIT_COMMIT",
        "SOURCE_COMMIT",
        "GITHUB_SHA",
        "CI_COMMIT_SHA",
        "CIRCLE_SHA1",
        "BUILDKITE_COMMIT",
    ] {
        println!("cargo:rerun-if-env-changed={}", key);
    }

    // When HEAD is `ref: refs/heads/...`, `.git/HEAD` does not change on new commits — only the
    // branch ref (or reflog) does. Watching those avoids a stale `DEN_GIT_COMMIT` under incremental builds.
    let git_dir = Path::new(manifest_dir).join("../.git");
    let head_path = git_dir.join("HEAD");
    if head_path.exists() {
        println!("cargo:rerun-if-changed={}", head_path.display());
        if let Ok(contents) = std::fs::read_to_string(&head_path) {
            let line = contents.trim();
            if let Some(rest) = line.strip_prefix("ref: ") {
                let ref_path = git_dir.join(rest);
                if ref_path.exists() {
                    println!("cargo:rerun-if-changed={}", ref_path.display());
                }
            }
        }
    }
    let logs_head = git_dir.join("logs/HEAD");
    if logs_head.exists() {
        println!("cargo:rerun-if-changed={}", logs_head.display());
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let build_start = Instant::now();
    println!("cargo:warning=Build script starting...");

    let manifest_dir = env::var("CARGO_MANIFEST_DIR")?;
    let git_commit = resolve_git_commit();
    println!("cargo:rustc-env=DEN_GIT_COMMIT={}", git_commit);
    emit_git_commit_rerun_hints(&manifest_dir);

    // Only run expensive embedding & DB migration steps when the `production`
    // feature is enabled. Cargo exposes enabled features to build scripts via
    // environment variables named CARGO_FEATURE_<FEATURE_NAME_UPPER>.
    let production_enabled = env::var_os("CARGO_FEATURE_PRODUCTION").is_some();

    // Always record the assets dir variable (cheap) but avoid any heavy work
    // unless production is explicitly requested.
    let _assets_dir = std::env::var("ASSETS_DIR").unwrap_or("src/web/assets".to_string());

    if production_enabled {
        let template_start = Instant::now();
        println!("cargo:warning=Production build: starting template embedding...");

        let templates_dir =
            std::env::var("TEMPLATES_DIR").unwrap_or("src/web/templates".to_string());
        minijinja_embed::embed_templates!(&templates_dir);

        let email_templates_dir =
            std::env::var("EMAIL_TEMPLATES_DIR").unwrap_or("src/core/email/templates".to_string());
        minijinja_embed::embed_templates!(&email_templates_dir, &[][..], "email");

        let api_templates_dir =
            std::env::var("API_TEMPLATES_DIR").unwrap_or("src/api/templates".to_string());
        minijinja_embed::embed_templates!(&api_templates_dir, &[][..], "api");

        println!(
            "cargo:warning=Template embedding completed in {:.2}s",
            template_start.elapsed().as_secs_f64()
        );
    } else {
        println!(
            "cargo:warning=Skipping template embedding; enable the 'production' feature to run it."
        );
    }

    println!(
        "cargo:warning=Build script completed in {:.2}s",
        build_start.elapsed().as_secs_f64()
    );
    Ok(())
}
