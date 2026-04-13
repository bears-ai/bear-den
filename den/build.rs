use std::env;
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let build_start = Instant::now();
    println!("cargo:warning=Build script starting...");

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
