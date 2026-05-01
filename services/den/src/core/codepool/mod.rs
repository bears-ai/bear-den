//! Letta Code **Codepool** harness (Node / SDK) — streaming execution tier for Den.

mod client;

pub use client::{
    BearRuntimeClient, CodePoolClient, CodepoolToolResultRequest, CodepoolToolResultResponse,
};
