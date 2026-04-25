//! Standalone API service (`src/api/`)
//!
//! This module provides an independent API service for apps built from this starter that can run
//! separately from or alongside the web service. It implements OAuth 2.0
//! authorization server functionality and external API access for third parties.
//!
//! # Architecture
//!
//! **HTTP surface in this starter:**
//! - **`src/api/`** (this module): standalone OAuth 2.0 authorization server, OpenAPI docs, and JSON API (`/v1.0/`, etc.).
//! - **`src/web/`**: server-rendered UI (Axum routes under `public`, `user`, `admin`, `home`, …)—add product-specific routes here, not a separate `web/api` tree.
//!
//! The API service is designed with the same architectural patterns as the web
//! service but maintains complete independence. This allows for:
//! - Separate deployment and scaling (port 3001 vs web service port 3000)
//! - Independent authentication and session management
//! - Dedicated API-focused middleware and error handling
//! - Future distribution across multiple crates
//!
//! # Endpoints Provided
//!
//! - **OAuth 2.0** (`/oauth/`): Complete authorization server (RFC 6749, RFC 7636)
//! - **API Documentation** (`/api-docs/`): OpenAPI documentation for external API
//! - **v1.0 API** (`/v1.0/`): External API endpoints for third-party access
//!
//! # OAuth 2.0 Provider
//!
//! The API service implements a complete OAuth 2.0 authorization server following
//! RFC 6749, including:
//! - Authorization endpoint for user consent
//! - Token endpoint for access token exchange
//! - User info endpoint for profile data
//! - PKCE support for enhanced security (RFC 7636)
//!
//! # Security
//!
//! - Integrates with existing axum-login authentication system
//! - Uses PostgreSQL session store shared with web service
//! - Implements proper CORS for cross-origin OAuth flows
//! - Follows "safe code" principles with comprehensive error handling
//!
//! # Deployment
//!
//! Can be run independently or alongside the web service:
//! - `SERVER_MODE=api` - Run only API service (port 3001)
//! - `SERVER_MODE=web` - Run only web service (port 3000)
//! - `SERVER_MODE=both` - Run both services simultaneously

pub mod docs;
pub mod oauth;
pub mod service;
pub mod templates;
pub mod v1;

// Re-export main API service creation function
pub use service::create_api_app;
