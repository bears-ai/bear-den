//! Storage I/O injection: registry authorization remains outside this boundary.

use super::ArtifactContentLocation;
use den_core::DenError;
use std::{future::Future, pin::Pin};

pub type ArtifactReadFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<u8>, DenError>> + Send + 'a>>;

pub trait ArtifactByteReader: Send + Sync {
    fn read<'a>(&'a self, location: &'a ArtifactContentLocation) -> ArtifactReadFuture<'a>;
}
