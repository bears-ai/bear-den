//! Binary entrypoint. Application logic lives in the `newapp` library crate ([`newapp::run`]).
use std::io::Error;

/// Main entry point (starter template; default package name `newapp`).
///
/// Enable services with `RUN_WEB`, `RUN_API`, `RUN_WORKERS` (see `README.md` and [`newapp::config::Config`]).
#[tokio::main]
async fn main() -> Result<(), Error> {
    newapp::run().await
}
