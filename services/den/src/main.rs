//! Binary entrypoint. Application logic lives in the `den` library crate ([`den::run`]).

/// Den control-plane binary (BEARS Phase 1).
///
/// Enable services with `RUN_WEB`, `RUN_API`, `RUN_WORKERS` (see `README.md` and [`den::config::Config`]).
#[tokio::main]
async fn main() {
    if let Err(e) = den::run().await {
        eprintln!("den: {e}");
        std::process::exit(1);
    }
}
