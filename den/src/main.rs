//! Binary entrypoint. Application logic lives in the `den` library crate ([`den::run`]).
use std::io::Error;

/// Den control-plane binary (BEARS Phase 1).
///
/// Enable services with `RUN_WEB`, `RUN_API`, `RUN_WORKERS` (see `README.md` and [`den::config::Config`]).
#[tokio::main]
async fn main() -> Result<(), Error> {
    den::run().await
}
