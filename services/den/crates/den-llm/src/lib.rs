//! `den-llm`: OpenAI-compatible streaming inference client (Bifrost / `LLM_API_URL`).
//!
//! Stable leaf crate (see `docs/roadmap/DEN_CRATE_SPLIT_PLAN.md`). Holds the LLM
//! client and request/response types; depends only on `den-core`. The SSE ->
//! `RuntimeStreamEvent` mapping lives in den-runtime to avoid a cycle.

mod client;
mod embeddings;
mod idle_byte_stream;
mod model_options;
pub mod model_registry;
mod transport_policy;

pub use client::{
    bifrost_key_selection_error, normalize_llm_model_handle, preferred_api_style_for_model,
    preferred_api_style_for_model_with_catalog_support, ChatCompletionRequest, ChatMessage,
    ChatToolCall, ChatToolCallFunction, LlmApiStyle, LlmClient, LlmOperation, LlmRequestTelemetry,
    LlmToolDefinition,
};
pub use embeddings::{BearEmbeddingCredential, EmbeddingClient};
pub use idle_byte_stream::byte_stream_with_idle_timeout;
pub use model_options::{ModelOption, ToolOption};
pub use model_registry::execution_fallback_model_handles;
pub use transport_policy::{primary_api_style_for_catalog_support, PrimaryTransportPreference};
