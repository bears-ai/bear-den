# Axum Handler Patterns

This document explains the correct patterns for implementing Axum handlers in this project, including route parameter extraction, request/response handling, and common pitfalls.

## Overview

This starter uses **Axum** for HTTP request handling. This provides:
- Type-safe request extraction
- Compile-time route validation
- Async handler support
- Middleware integration

## Route Parameter Extraction

### Path Parameters

**CRITICAL**: The colon-prefixed `:placeholder` syntax is **obsolete and will not work** in Axum. Use the `Path` extractor instead.

**❌ WRONG** - Obsolete syntax (does not work):
```rust
async fn handler(
    Path(id): Path<i32>,  // This is correct
    // But the route definition should NOT use :id
) -> Result<Json<Response>, CustomError> {
    // ...
}
```

**✅ CORRECT** - Modern Axum pattern:
```rust
// Route definition
.route("/v1.0/resources/:id", put(update_resource))  // ❌ WRONG - colon syntax doesn't work
.route("/v1.0/resources/{id}", put(update_resource))  // ✅ CORRECT - curly braces

// Handler function
async fn update_resource(
    Path(id): Path<i32>,  // Extracts {id} from the route
    // ...
) -> Result<Json<ResourceBody>, CustomError> {
    // ...
}
```

**Pattern**:
- Route definition uses **curly braces**: `{id}`, `{username}`, `{slug}`, etc.
- Handler uses **`Path(param): Path<Type>`** to extract the parameter
- The parameter name in `Path(param)` must match the route placeholder name

**Example (illustrative)**:
```rust
// e.g. PUT /v1.0/some-resource/{id}
async fn update_resource(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    Json(request): Json<UpdateBody>,
) -> Result<Json<ResponseBody>, CustomError> {
    let _ = (state, headers, id, request);
    todo!()
}
```

### Multiple Path Parameters

```rust
// Route definition
.route("/admin/users/{id}/edit", get(edit_user))

// Handler
async fn edit_user(
    Path(id): Path<i32>,
    // ...
) -> Result<Response, CustomError> {
    // ...
}
```

For multiple parameters, use a struct:

```rust
#[derive(Deserialize)]
struct UserPathParams {
    id: i32,
    action: String,
}

// Route: /admin/users/{id}/{action}
async fn user_action(
    Path(params): Path<UserPathParams>,
) -> Result<Response, CustomError> {
    let user_id = params.id;
    let action = params.action;
    // ...
}
```

## Request Extractors

### JSON Body

```rust
use axum::extract::Json;

async fn create_resource(
    Json(request): Json<CreateRequest>,
) -> Result<Json<Response>, CustomError> {
    // request is automatically deserialized from JSON body
}
```

### Query Parameters

```rust
use axum::extract::Query;

#[derive(Deserialize)]
struct FilterParams {
    q: Option<String>,
    page: Option<u32>,
}

async fn list_resources(
    Query(params): Query<FilterParams>,
) -> Result<Json<Response>, CustomError> {
    // params.q, params.page available
}
```

### State

```rust
use axum::extract::State;

async fn handler(
    State(state): State<AppState>,
) -> Result<Response, CustomError> {
    // Access state.sqlx_pool, state.redis, etc.
}
```

### Headers

```rust
use axum::http::HeaderMap;

async fn handler(
    headers: HeaderMap,
) -> Result<Response, CustomError> {
    let auth_header = headers.get("authorization");
    // ...
}
```

### Multiple Extractors

You can combine multiple extractors in any order:

```rust
async fn handler(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Path(id): Path<i32>,
    Query(params): Query<FilterParams>,
    Json(body): Json<RequestBody>,
) -> Result<Json<Response>, CustomError> {
    // All extractors available
}
```

## Response Types

### JSON Response

```rust
use axum::response::Json;

async fn handler() -> Result<Json<ResponseData>, CustomError> {
    Ok(Json(ResponseData { /* ... */ }))
}
```

### Redirect

```rust
use axum::response::Redirect;

async fn handler() -> Result<Redirect, CustomError> {
    Ok(Redirect::to("/new-location"))
}
```

### HTML/Template Response

```rust
use crate::web::render_template;

async fn handler(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Response, CustomError> {
    render_template(
        state.template_env.clone(),
        "template_name.html",
        context! { /* ... */ },
        uri,
    )
    .await
}
```

## Common Patterns

### Authentication Check

```rust
use axum_login::login_required;

// Apply to route
.route("/protected", get(handler))
.route_layer(login_required!(Backend, login_url = "/login"))
```

### OAuth / bearer tokens

Validate JWT or bearer tokens in your API layer (see `src/api/oauth/` and protected routes). Handlers typically extract `HeaderMap` or a custom extractor, then map failures to `CustomError`.

### Error Handling

All handlers should return `Result<T, CustomError>`:

```rust
async fn handler() -> Result<Json<Response>, CustomError> {
    // Operations that can fail
    let result = some_operation().await?;  // ? propagates CustomError

    Ok(Json(Response { /* ... */ }))
}
```

## Route Definition

### Basic Route

```rust
use axum::routing::{get, post, put, delete};

Router::new()
    .route("/path", get(handler))
    .route("/path", post(handler))
```

### Route with Path Parameters

```rust
Router::new()
    .route("/users/{id}", get(get_user))
    .route("/users/{id}", put(update_user))
    .route("/users/{id}", delete(delete_user))
```

### Nested Routes

```rust
Router::new()
    .nest("/api/v1", api_v1_router())
    .nest("/admin", admin_router())
```

## Common Pitfalls

### ❌ Using Colon Syntax in Routes

**Wrong**:
```rust
.route("/users/:id", get(handler))  // Won't work!
```

**Correct**:
```rust
.route("/users/{id}", get(handler))  // Use curly braces
```

### ❌ Mismatched Parameter Names

**Wrong**:
```rust
.route("/users/{id}", get(handler))

async fn handler(Path(user_id): Path<i32>) {  // Name doesn't match!
    // ...
}
```

**Correct**:
```rust
.route("/users/{id}", get(handler))

async fn handler(Path(id): Path<i32>) {  // Matches route placeholder
    // ...
}
```

### ❌ Missing Extractors

**Wrong**:
```rust
async fn handler() {
    // Trying to access state without extracting it
    state.sqlx_pool.query()  // Compile error!
}
```

**Correct**:
```rust
async fn handler(State(state): State<AppState>) {
    state.sqlx_pool.query()  // Works!
}
```

## Examples from Codebase

### API-style handler with `State` + `Json`

```rust
async fn register_user(
    State(state): State<ApiState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, CustomError> {
    let _ = (state, body);
    todo!()
}
```

### Web admin handler with `Path` + `Form`

```rust
// Pattern from src/web/admin/users.rs — edit user POST
async fn edit_user_post(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession<Backend>,
    Form(form): Form<UserEditForm>,
) -> Result<Response, CustomError> {
    let _ = (id, state, auth_session, form);
    todo!()
}
```

## Summary

- ✅ Use **curly braces** `{id}` in route definitions
- ✅ Use **`Path(param): Path<Type>`** to extract path parameters
- ✅ Parameter names must match between route and extractor
- ❌ **Never use colon syntax** `:id` in routes (obsolete, doesn't work)
- ✅ Combine extractors as needed: `State`, `Path`, `Query`, `Json`, `HeaderMap`
- ✅ Always return `Result<T, CustomError>` for error handling

