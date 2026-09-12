//! Binary entrypoint. Application logic lives in the `den` library crate ([`den::run`]).

use den::seeds::SeedProfile;

/// Den control-plane binary (BEARS Phase 1).
///
/// Enable services with `RUN_WEB`, `RUN_API`, and `RUN_WORKERS`.
///
/// Current `RUN_WORKERS=true` behavior starts the in-process memory-curate worker,
/// which polls queued `memory_curate` reflection runs and processes them until shutdown.
/// See `README.md`, `.env.example`, and [`den::config::Config`].
#[tokio::main]
async fn main() {
    if let Err(e) = run_main().await {
        eprintln!("den: {e:#}");
        std::process::exit(1);
    }
}

async fn run_main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("migrate") => {
            den::migrate_only().await?;
            Ok(())
        }
        Some("serve") => {
            den::serve_without_migrations().await?;
            Ok(())
        }
        Some("seed") => run_seed(&args).await,
        Some("reindex") => match den::reindex::parse_args(&args)? {
            Some(target) => den::reindex::run_reindex(target).await,
            None => Ok(()),
        },
        Some("import-legacy-memory") => match den::import_legacy_memory::parse_args(&args)? {
            Some(parsed) => den::import_legacy_memory::run_import_legacy_memory(parsed).await,
            None => Ok(()),
        },
        _ => {
            den::run().await?;
            Ok(())
        }
    }
}

async fn run_seed(args: &[String]) -> anyhow::Result<()> {
    let mut profile = SeedProfile::Smoke;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--profile" => {
                let raw = args
                    .get(i + 1)
                    .ok_or_else(|| anyhow::anyhow!("--profile requires a value"))?;
                profile = SeedProfile::parse(raw)?;
                i += 2;
            }
            "--help" | "-h" => {
                println!("Usage: den seed [--profile smoke|minimal]");
                return Ok(());
            }
            other => anyhow::bail!("unknown seed argument {other:?}"),
        }
    }

    dotenvy::dotenv().ok();
    let database_url = std::env::var("DATABASE_URL")?;
    let report = den::seeds::seed_database_url(&database_url, profile).await?;
    println!(
        "seeded profile={} user={}({}) bear={}({})",
        report.profile.as_str(),
        report.username,
        report.user_id,
        report.bear_slug,
        report.bear_id
    );
    Ok(())
}
