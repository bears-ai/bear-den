//! One process-wide budget for hashing, ZIP reads and decompression. The permit
//! moves into the blocking closure, then stays with its output through creation.
use crate::errors::CustomError;
use std::sync::{Arc, OnceLock};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const CONCURRENT_JOBS: usize = 2;
static IMPORT_JOBS: OnceLock<Arc<Semaphore>> = OnceLock::new();

pub(super) async fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CustomError> + Send + 'static,
) -> Result<(T, OwnedSemaphorePermit), CustomError> {
    let semaphore = IMPORT_JOBS
        .get_or_init(|| Arc::new(Semaphore::new(CONCURRENT_JOBS)))
        .clone();
    run_with(semaphore, work).await
}

async fn run_with<T: Send + 'static>(
    semaphore: Arc<Semaphore>,
    work: impl FnOnce() -> Result<T, CustomError> + Send + 'static,
) -> Result<(T, OwnedSemaphorePermit), CustomError> {
    let permit = semaphore.acquire_owned().await.map_err(|_| {
        CustomError::System("Import processing is unavailable; retry shortly.".into())
    })?;
    tokio::task::spawn_blocking(move || {
        // A cancelled HTTP future drops its JoinHandle, not this permit. The
        // blocking operation and its file guards remain protected until done.
        let result = work()?;
        Ok::<_, CustomError>((result, permit))
    })
    .await
    .map_err(|_| {
        CustomError::System(
            "Import processing was interrupted. Check the import outcome before retrying.".into(),
        )
    })?
}

#[cfg(test)]
#[path = "tests/import_jobs.rs"]
mod tests;
