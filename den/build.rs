use std::env;
use std::time::Instant;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// UTC time when the build script ran, RFC 3339 (e.g. `2026-04-21T12:34:56Z`).
/// If `SOURCE_DATE_EPOCH` is set (seconds since Unix epoch), use that for reproducible builds.
fn build_time_utc_rfc3339() -> String {
    if let Ok(epoch) = env::var("SOURCE_DATE_EPOCH") {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let build_start = Instant::now();
    println!("cargo:warning=Build script starting...");

    let built_at = build_time_utc_rfc3339();
    println!("cargo:rustc-env=DEN_BUILT_AT_UTC={}", built_at);

    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

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
