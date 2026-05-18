//! Web-layer data-source ports used to keep Axum handlers/templates real while allowing
//! explicitly feature-gated fixture-backed integration data in development.
//!
//! These ports sit below `src/web/*` orchestration and above external integration clients such as
//! Letta, MemFS Manager, and Codepool. The goal is to support browser/UI smoke testing without
//! changing the shape of the web routes or templating layer.

pub mod chat_transport;
pub mod letta;
pub mod memory;

#[cfg(feature = "web-ui-fixtures")]
pub mod fixtures;

pub use chat_transport::{RealWebChatTransportDataSource, WebChatTransportDataSource};
pub use letta::{
    RealWebLettaDataSource, WebConversationRow, WebConversationSnapshot, WebLettaDataSource,
};
pub use memory::{RealWebMemoryDataSource, WebMemoryDataSource};
