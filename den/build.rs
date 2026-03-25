use std::env;
use std::time::Instant;

use sqlx::postgres::PgPoolOptions;
use tokio::runtime::Runtime;

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

        // Run and embed SQLx migrations only in production builds
        let migration_start = Instant::now();
        println!("cargo:warning=Production build: starting SQLx migrations...");

        let db_url = match env::var_os("DATABASE_URL") {
            Some(v) => v.into_string().unwrap(),
            None => {
                // In production builds this should be set; fail fast with a clear
                // message if not present.
                panic!("DATABASE_URL must be defined for production builds");
            }
        };

        let runtime = Runtime::new().unwrap();
        runtime.block_on(async {
            let pool_start = Instant::now();
            let sqlx_pool = PgPoolOptions::new()
                .max_connections(5)
                .acquire_timeout(std::time::Duration::from_secs(3))
                .connect(&db_url)
                .await
                .expect("database should be available");
            println!(
                "cargo:warning=Database connection established in {:.2}s",
                pool_start.elapsed().as_secs_f64()
            );

            let migrate_start = Instant::now();
            // embed and execute migrations
            sqlx::migrate!()
                .set_ignore_missing(true)
                .run(&sqlx_pool)
                .await
                .unwrap();
            println!(
                "cargo:warning=Migrations executed in {:.2}s",
                migrate_start.elapsed().as_secs_f64()
            );
        });

        println!(
            "cargo:warning=SQLx migrations completed in {:.2}s",
            migration_start.elapsed().as_secs_f64()
        );
    } else {
        println!(
            "cargo:warning=Skipping template embedding and SQLx migrations; enable the 'production' feature to run them."
        );
    }

    println!(
        "cargo:warning=Build script completed in {:.2}s",
        build_start.elapsed().as_secs_f64()
    );
    Ok(())
}
