// ROUTES: When modifying routes in this file, update /src/web/ROUTES.md
use serde::{Deserialize, Serialize};

use axum::{
    Router, debug_handler,
    extract::{Path, Query, State},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use axum_extra::extract::Form;
use axum_extra::routing::RouterExt;

use validator::{Validate, ValidationError, ValidationErrors};

use minijinja::context;

use crate::{
    api::oauth::{
        AccessTokenWithContext, OAuthScope, db as oauth_db,
        utils::{
            generate_access_token, generate_client_id, generate_client_secret,
            generate_pkce_code_challenge, generate_pkce_code_verifier, hash_client_secret,
            scopes_from_json, validate_code_challenge_method, validate_pkce, validate_redirect_uri,
            validate_scopes_with_conflict_detection,
        },
    },
    auth_backend::AuthSession,
    errors::CustomError,
    web::{self, AppState},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route_with_tsr("/oauth_clients/", get(oauth_clients_list))
        .route_with_tsr(
            "/oauth_clients/add",
            get(add_oauth_client_view).post(add_oauth_client_action),
        )
        .route_with_tsr("/oauth_clients/{id}", get(view_oauth_client))
        .route_with_tsr(
            "/oauth_clients/{id}/edit",
            get(edit_oauth_client_view).post(edit_oauth_client_action),
        )
        .route_with_tsr(
            "/oauth_clients/{id}/regenerate_secret",
            post(regenerate_client_secret_action),
        )
        .route_with_tsr(
            "/oauth_clients/{id}/toggle_trusted",
            post(toggle_trusted_status_action),
        )
        .route_with_tsr(
            "/oauth_clients/{id}/deactivate",
            post(deactivate_oauth_client_action),
        )
        // PKCE testing routes
        .route_with_tsr(
            "/oauth_clients/{id}/pkce-test",
            get(pkce_test_view).post(pkce_test_action),
        )
        // OAuth token management routes
        .route_with_tsr("/oauth_tokens/", get(oauth_tokens_list))
        .route_with_tsr("/oauth_tokens/{token_id}", get(view_oauth_token))
        .route_with_tsr(
            "/oauth_tokens/{token_id}/revoke",
            post(revoke_oauth_token_action),
        )
        .route_with_tsr(
            "/oauth_tokens/generate",
            get(generate_token_view).post(generate_token_action),
        )
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct OAuthClientForm {
    #[validate(length(min = 1, max = 255))]
    name: String,
    #[validate(custom(function = "validate_redirect_uris"))]
    redirect_uris: String, // Newline-separated URIs
    #[validate(custom(function = "validate_scopes"))]
    scopes: Vec<String>, // Selected scopes
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct PKCETestForm {
    #[validate(length(min = 1, message = "Code verifier is required"))]
    code_verifier: String,
    #[validate(length(min = 1, message = "Code challenge is required"))]
    code_challenge: String,
    #[validate(length(min = 1, message = "Challenge method is required"))]
    code_challenge_method: String,
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct GenerateTokenForm {
    #[validate(length(min = 1, message = "Client is required"))]
    client_id: String,
    #[validate(length(min = 1, message = "User is required"))]
    user_id: String,
    #[validate(custom(function = "validate_scopes"))]
    scopes: Vec<String>,
    #[validate(range(
        min = 1,
        max = 720,
        message = "Expiration must be between 1 and 720 hours"
    ))]
    expires_in: i32,
}

#[derive(Validate, Serialize, Deserialize)]
pub struct NewOAuthClientForm {
    #[validate(length(min = 1, max = 255))]
    name: String,
    #[validate(custom(function = "validate_redirect_uris"))]
    redirect_uris: String, // Newline-separated URIs
    #[validate(custom(function = "validate_scopes"))]
    scopes: Vec<String>, // Selected scopes
    #[serde(default)]
    public: bool, // Whether this is a public client (uses PKCE)
}

// Custom validation for redirect URIs
fn validate_redirect_uris(redirect_uris: &str) -> Result<(), ValidationError> {
    if redirect_uris.trim().is_empty() {
        return Err(ValidationError::new("redirect_uris_required").with_message(
            std::borrow::Cow::Borrowed("At least one redirect URI is required"),
        ));
    }

    for uri in redirect_uris.lines() {
        let uri = uri.trim();
        if !uri.is_empty() && !validate_redirect_uri(uri) {
            return Err(ValidationError::new("invalid_redirect_uri_format")
                .with_message(std::borrow::Cow::Borrowed("Invalid redirect URI format")));
        }
    }
    Ok(())
}

// Custom validation for scopes
fn validate_scopes(scopes: &[String]) -> Result<(), ValidationError> {
    if scopes.is_empty() {
        return Err(ValidationError::new("scopes_required").with_message(
            std::borrow::Cow::Borrowed("At least one scope must be selected"),
        ));
    }

    for scope_str in scopes {
        if OAuthScope::from_str(scope_str).is_none() {
            return Err(ValidationError::new("invalid_scope")
                .with_message(std::borrow::Cow::Borrowed("Invalid scope")));
        }
    }
    Ok(())
}

// Convert database record to form
impl TryFrom<crate::api::oauth::OAuthClient> for OAuthClientForm {
    type Error = CustomError;

    fn try_from(client: crate::api::oauth::OAuthClient) -> Result<Self, Self::Error> {
        // Parse redirect URIs from JSON array
        let redirect_uris = match client.redirect_uris.as_array() {
            Some(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            None => String::new(),
        };

        // Parse scopes from JSON array
        let scopes = scopes_from_json(&client.scopes)
            .map_err(|_| CustomError::Parsing("Invalid scopes format".to_string()))?
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();

        Ok(Self {
            name: client.name,
            redirect_uris,
            scopes,
        })
    }
}

async fn oauth_clients_list(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let clients = oauth_db::list_oauth_clients(&state.sqlx_pool).await?;

    web::render_template(
        state.template_env,
        "admin/oauth_clients/list.html",
        auth_session,
        context! {
            clients
        },
    )
    .await
}

async fn add_oauth_client_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let available_scopes: Vec<String> = OAuthScope::all()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();

    web::render_template(
        state.template_env,
        "admin/oauth_clients/add.html",
        auth_session,
        context! {
            available_scopes
        },
    )
    .await
}

#[debug_handler]
pub async fn add_oauth_client_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<NewOAuthClientForm>,
) -> Result<Response, CustomError> {
    tracing::debug!(
        "Received OAuth client creation request: name={}, redirect_uris={}, scopes={:?}",
        form.name,
        form.redirect_uris,
        form.scopes
    );

    let mut validation_errors = ValidationErrors::new();
    if let Err(form_validation_errors) = form.validate() {
        tracing::warn!("Form validation failed: {:?}", form_validation_errors);
        validation_errors = form_validation_errors;
    }

    if validation_errors.is_empty() {
        // Generate client credentials
        let client_id = generate_client_id();

        // For public clients, don't generate a client secret
        // For confidential clients, generate and hash the secret
        let (client_secret, client_secret_hash) = if form.public {
            (None, None)
        } else {
            let secret = generate_client_secret();
            let hash = hash_client_secret(&secret)
                .map_err(|e| CustomError::System(format!("Failed to hash client secret: {e}")))?;
            (Some(secret), Some(hash))
        };

        // Parse redirect_uris into JSON array
        let redirect_uris: Vec<String> = form
            .redirect_uris
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let redirect_uris_json = serde_json::to_value(redirect_uris)
            .map_err(|e| CustomError::Parsing(format!("Failed to serialize redirect URIs: {e}")))?;

        // Parse scopes
        let scopes: Vec<OAuthScope> = form
            .scopes
            .iter()
            .filter_map(|s| OAuthScope::from_str(s))
            .collect();

        // Enhanced scope validation with conflict detection
        let scope_warnings = validate_scopes_with_conflict_detection(&scopes)
            .map_err(|e| CustomError::ValidationError(format!("Scope validation failed: {}", e)))?;

        // Log warnings but don't fail on them - warnings are informational only
        if !scope_warnings.is_empty() {
            tracing::warn!("Scope validation warnings: {:?}", scope_warnings);
        }

        tracing::info!(
            "Creating OAuth client: name={}, client_id={}, public={}",
            form.name,
            client_id,
            form.public
        );

        let client_db_id = oauth_db::create_oauth_client(
            &state.sqlx_pool,
            &client_id,
            client_secret_hash.as_deref(),
            &form.name,
            &redirect_uris_json,
            &scopes,
            form.public,
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to create OAuth client: {:?}", e);
            e
        })?;

        tracing::info!(
            "Successfully created OAuth client: id={}, client_id={}, public={}",
            client_db_id,
            client_id,
            form.public
        );

        // Redirect to view page with success message
        // For public clients, don't show client_secret in URL
        let redirect_url = if let Some(secret) = client_secret {
            format!("/admin/oauth_clients/{client_db_id}?created=true&client_secret={secret}")
        } else {
            format!("/admin/oauth_clients/{client_db_id}?created=true")
        };
        Ok(Redirect::to(&redirect_url).into_response())
    } else {
        let available_scopes: Vec<String> = OAuthScope::all()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();

        // Process validation errors for template (replacing map filter)
        let redirect_uris_errors: Vec<String> = validation_errors
            .field_errors()
            .get("redirect_uris")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let scopes_errors: Vec<String> = validation_errors
            .field_errors()
            .get("scopes")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        web::render_template(
            state.template_env,
            "admin/oauth_clients/add.html",
            auth_session,
            context! {
                errors => validation_errors,
                redirect_uris_errors,
                scopes_errors,
                client => form,
                available_scopes
            },
        )
        .await
    }
}

#[derive(Deserialize)]
struct ViewQueryParams {
    created: Option<String>,
    regenerated: Option<String>,
    client_secret: Option<String>,
}

async fn view_oauth_client(
    Path(id): Path<i32>,
    Query(params): Query<ViewQueryParams>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let client = oauth_db::get_oauth_client_by_id(&state.sqlx_pool, id)
        .await?
        .ok_or_else(|| CustomError::NotFound("OAuth client not found".to_string()))?;

    // Parse scopes for display - convert to strings for template
    let scopes_enum = scopes_from_json(&client.scopes)
        .map_err(|_| CustomError::Parsing("Invalid scopes format".to_string()))?;
    let scopes: Vec<String> = scopes_enum.iter().map(|s| s.as_str().to_string()).collect();

    // Parse redirect URIs for display
    let redirect_uris: Vec<String> = match client.redirect_uris.as_array() {
        Some(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect(),
        None => vec![],
    };

    // Extract query parameters for success messages
    let show_created = params.created.as_deref() == Some("true");
    let show_regenerated = params.regenerated.as_deref() == Some("true");
    let client_secret = params.client_secret;

    web::render_template(
        state.template_env,
        "admin/oauth_clients/view.html",
        auth_session,
        context! {
            client,
            scopes,
            redirect_uris,
            show_created,
            show_regenerated,
            client_secret,
        },
    )
    .await
}

async fn edit_oauth_client_view(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let client = oauth_db::get_oauth_client_by_id(&state.sqlx_pool, id)
        .await?
        .ok_or_else(|| CustomError::NotFound("OAuth client not found".to_string()))?;

    let client_form: OAuthClientForm = client.try_into()?;
    let available_scopes: Vec<String> = OAuthScope::all()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();

    web::render_template(
        state.template_env,
        "admin/oauth_clients/edit.html",
        auth_session,
        context! {
            id,
            client => client_form,
            available_scopes
        },
    )
    .await
}

#[debug_handler]
pub async fn edit_oauth_client_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<OAuthClientForm>,
) -> Result<Response, CustomError> {
    let mut validation_errors = ValidationErrors::new();
    if let Err(form_validation_errors) = form.validate() {
        validation_errors = form_validation_errors;
    }

    if validation_errors.is_empty() {
        // Parse redirect URIs into JSON array
        let redirect_uris: Vec<String> = form
            .redirect_uris
            .lines()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let redirect_uris_json = serde_json::to_value(redirect_uris)
            .map_err(|e| CustomError::Parsing(format!("Failed to serialize redirect URIs: {e}")))?;

        // Parse scopes
        let scopes: Vec<OAuthScope> = form
            .scopes
            .iter()
            .filter_map(|s| OAuthScope::from_str(s))
            .collect();

        // Enhanced scope validation with conflict detection
        let scope_warnings = validate_scopes_with_conflict_detection(&scopes)
            .map_err(|e| CustomError::ValidationError(format!("Scope validation failed: {}", e)))?;

        // Log warnings but don't fail on them - warnings are informational only
        if !scope_warnings.is_empty() {
            tracing::warn!("Scope validation warnings: {:?}", scope_warnings);
        }

        oauth_db::update_oauth_client(
            &state.sqlx_pool,
            id,
            &form.name,
            &redirect_uris_json,
            &scopes,
        )
        .await?;

        Ok(Redirect::to(&format!("/admin/oauth_clients/{id}")).into_response())
    } else {
        let available_scopes: Vec<String> = OAuthScope::all()
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();

        // Process validation errors for template (replacing map filter)
        let redirect_uris_errors: Vec<String> = validation_errors
            .field_errors()
            .get("redirect_uris")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let scopes_errors: Vec<String> = validation_errors
            .field_errors()
            .get("scopes")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        web::render_template(
            state.template_env,
            "admin/oauth_clients/edit.html",
            auth_session,
            context! {
                id,
                errors => validation_errors,
                redirect_uris_errors,
                scopes_errors,
                client => form,
                available_scopes
            },
        )
        .await
    }
}

async fn pkce_test_view(
    Path(client_id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    // Get the client to verify it exists
    let client = oauth_db::get_oauth_client_by_id(&state.sqlx_pool, client_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("OAuth client not found".to_string()))?;

    // Generate a sample PKCE code verifier and challenge for demonstration
    let sample_verifier = generate_pkce_code_verifier();
    let sample_challenge = generate_pkce_code_challenge(&sample_verifier);

    web::render_template(
        state.template_env,
        "admin/oauth_clients/pkce_test.html",
        auth_session,
        context! {
            client_id,
            client_name => client.name,
            sample_verifier,
            sample_challenge,
            sample_method => "S256",
            test_result => None::<PKCETestResult>,
            form_data => None::<PKCETestForm>,
            errors => None::<Option<minijinja::value::Value>>,
        },
    )
    .await
}

#[derive(Serialize, Debug)]
pub struct PKCETestResult {
    pub success: bool,
    pub message: String,
    pub validation_details: String,
}

#[debug_handler]
pub async fn pkce_test_action(
    Path(client_id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<PKCETestForm>,
) -> Result<Response, CustomError> {
    let mut validation_errors = ValidationErrors::new();
    if let Err(form_validation_errors) = form.validate() {
        validation_errors = form_validation_errors;
    }

    // Validate code_challenge_method
    if !validate_code_challenge_method(&form.code_challenge_method) {
        validation_errors.add(
            "code_challenge_method",
            ValidationError::new("invalid_challenge_method").with_message(
                std::borrow::Cow::Borrowed("Invalid challenge method. Must be 'S256' or 'plain'"),
            ),
        );
    }

    if validation_errors.is_empty() {
        // Test PKCE validation
        let result = validate_pkce(
            &form.code_verifier,
            &form.code_challenge,
            &form.code_challenge_method,
        );

        let test_result = match result {
            Ok(_) => PKCETestResult {
                success: true,
                message: "PKCE validation successful!".to_string(),
                validation_details: format!(
                    "Code verifier '{}' successfully validated against challenge '{}' using method '{}'",
                    &form.code_verifier, &form.code_challenge, &form.code_challenge_method
                ),
            },
            Err(e) => PKCETestResult {
                success: false,
                message: "PKCE validation failed!".to_string(),
                validation_details: format!("Error: {}", e),
            },
        };

        web::render_template(
            state.template_env,
            "admin/oauth_clients/pkce_test.html",
            auth_session,
            context! {
                client_id,
                client_name => oauth_db::get_oauth_client_by_id(&state.sqlx_pool, client_id)
                    .await?.map(|c| c.name).unwrap_or_default(),
                sample_verifier => generate_pkce_code_verifier(),
                sample_challenge => generate_pkce_code_challenge(&form.code_verifier),
                sample_method => "S256",
                test_result => Some(test_result),
                form_data => Some(form),
                errors => None::<Option<minijinja::value::Value>>,
            },
        )
        .await
    } else {
        // Process validation errors for template
        let code_verifier_errors: Vec<String> = validation_errors
            .field_errors()
            .get("code_verifier")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let code_challenge_errors: Vec<String> = validation_errors
            .field_errors()
            .get("code_challenge")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let code_challenge_method_errors: Vec<String> = validation_errors
            .field_errors()
            .get("code_challenge_method")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        web::render_template(
            state.template_env,
            "admin/oauth_clients/pkce_test.html",
            auth_session,
            context! {
                client_id,
                client_name => oauth_db::get_oauth_client_by_id(&state.sqlx_pool, client_id)
                    .await?.map(|c| c.name).unwrap_or_default(),
                sample_verifier => generate_pkce_code_verifier(),
                sample_challenge => generate_pkce_code_challenge(&form.code_verifier),
                sample_method => "S256",
                test_result => None::<PKCETestResult>,
                form_data => Some(form),
                errors => Some(context! {
                    code_verifier => code_verifier_errors,
                    code_challenge => code_challenge_errors,
                    code_challenge_method => code_challenge_method_errors,
                }),
            },
        )
        .await
    }
}

pub async fn regenerate_client_secret_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    // Generate new client secret
    let new_client_secret = generate_client_secret();
    let new_client_secret_hash = hash_client_secret(&new_client_secret)
        .map_err(|e| CustomError::System(format!("Failed to hash client secret: {e}")))?;

    // Update the client secret in database
    sqlx::query!(
        "UPDATE oauth_clients SET client_secret = $1, updated_at = NOW() WHERE id = $2",
        new_client_secret_hash,
        id
    )
    .execute(&state.sqlx_pool)
    .await?;

    // Redirect back to view page with the new secret
    Ok(Redirect::to(&format!(
        "/admin/oauth_clients/{id}?regenerated=true&client_secret={new_client_secret}"
    )))
}

pub async fn toggle_trusted_status_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    // Get current client to check current trusted status
    let client = oauth_db::get_oauth_client_by_id(&state.sqlx_pool, id)
        .await?
        .ok_or_else(|| CustomError::NotFound("OAuth client not found".to_string()))?;

    // Toggle the trusted status
    let new_trusted_status = !client.trusted;
    oauth_db::update_oauth_client_trusted_status(&state.sqlx_pool, id, new_trusted_status).await?;

    // Redirect back to view page
    Ok(Redirect::to(&format!("/admin/oauth_clients/{id}")))
}

pub async fn deactivate_oauth_client_action(
    Path(id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    oauth_db::deactivate_oauth_client(&state.sqlx_pool, id).await?;

    // Redirect to clients list
    Ok(Redirect::to("/admin/oauth_clients/"))
}

async fn oauth_tokens_list(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let tokens = oauth_db::list_all_access_tokens_with_context(&state.sqlx_pool).await?;

    web::render_template(
        state.template_env,
        "admin/oauth_tokens/list.html",
        auth_session,
        context! {
            tokens
        },
    )
    .await
}

#[derive(Validate, Serialize, Deserialize, Debug)]
pub struct OAuthTokenForm {
    #[validate(length(min = 1, max = 255))]
    client_id: String,
    #[validate(length(min = 1, max = 255))]
    client_secret: String,
    #[validate(length(min = 1, max = 255))]
    #[serde(rename = "grant_type")]
    grant_type_: String,
    #[validate(length(min = 1, max = 255))]
    scope: String,
}

#[derive(Validate, Serialize, Deserialize)]
pub struct NewOAuthTokenForm {
    #[validate(length(min = 1, max = 255))]
    client_id: String,
    #[validate(length(min = 1, max = 255))]
    client_secret: String,
    #[validate(length(min = 1, max = 255))]
    #[serde(rename = "grant_type")]
    grant_type_: String,
    #[validate(length(min = 1, max = 255))]
    scope: String,
}

async fn view_oauth_token(
    Path(token_id): Path<i32>,
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    let token = oauth_db::get_oauth_token_by_id(&state.sqlx_pool, token_id)
        .await?
        .ok_or_else(|| CustomError::NotFound("OAuth token not found".to_string()))?;

    web::render_template(
        state.template_env,
        "admin/oauth_tokens/view.html",
        auth_session,
        context! {
            token
        },
    )
    .await
}

pub async fn revoke_oauth_token_action(
    Path(token_id): Path<i32>,
    State(state): State<AppState>,
) -> Result<Redirect, CustomError> {
    // Revoke the token (soft delete)
    oauth_db::revoke_oauth_token(&state.sqlx_pool, token_id).await?;

    // Redirect to tokens list
    Ok(Redirect::to("/admin/oauth_tokens/"))
}

async fn generate_token_view(
    State(state): State<AppState>,
    auth_session: AuthSession,
) -> Result<Response, CustomError> {
    // Get all active OAuth clients
    let clients = oauth_db::list_oauth_clients(&state.sqlx_pool).await?;

    // Get all active users
    let users = sqlx::query_as::<_, (i32, String, String, String)>(
        r#"
        SELECT id, username, display_name, email
        FROM users
        WHERE active = true
        ORDER BY username
        "#,
    )
    .fetch_all(&state.sqlx_pool)
    .await?;

    let available_scopes: Vec<String> = OAuthScope::all()
        .iter()
        .map(|s| s.as_str().to_string())
        .collect();

    web::render_template(
        state.template_env,
        "admin/oauth_tokens/generate.html",
        auth_session,
        context! {
            clients,
            users,
            available_scopes,
            form_data => None::<GenerateTokenForm>,
            errors => None::<Option<minijinja::value::Value>>,
            generated_token => None::<AccessTokenWithContext>,
        },
    )
    .await
}

#[debug_handler]
pub async fn generate_token_action(
    State(state): State<AppState>,
    auth_session: AuthSession,
    Form(form): Form<GenerateTokenForm>,
) -> Result<Response, CustomError> {
    let mut validation_errors = ValidationErrors::new();
    if let Err(form_validation_errors) = form.validate() {
        validation_errors = form_validation_errors;
    }

    if validation_errors.is_empty() {
        // Parse client_id and user_id
        let client_id = form
            .client_id
            .parse::<i32>()
            .map_err(|_| CustomError::ValidationError("Invalid client ID".to_string()))?;

        let user_id = form
            .user_id
            .parse::<i32>()
            .map_err(|_| CustomError::ValidationError("Invalid user ID".to_string()))?;

        // Validate that the client exists and is active
        let client = oauth_db::get_oauth_client_by_id(&state.sqlx_pool, client_id)
            .await?
            .ok_or_else(|| CustomError::NotFound("OAuth client not found".to_string()))?;

        if !client.active {
            return Err(CustomError::ValidationError(
                "Client is not active".to_string(),
            ));
        }

        // Validate that the user exists
        let user_exists = sqlx::query_scalar::<_, bool>(
            r#"SELECT EXISTS(SELECT 1 FROM users WHERE id = $1 AND active = true)"#,
        )
        .bind(user_id)
        .fetch_one(&state.sqlx_pool)
        .await?;

        if !user_exists {
            return Err(CustomError::ValidationError("User not found".to_string()));
        }

        // Parse scopes
        let scopes: Vec<OAuthScope> = form
            .scopes
            .iter()
            .filter_map(|s| OAuthScope::from_str(s))
            .collect();

        // Validate scopes against client's allowed scopes
        let client_scopes = scopes_from_json(&client.scopes)
            .map_err(|_| CustomError::Parsing("Invalid client scopes format".to_string()))?;

        if !crate::api::oauth::utils::validate_scopes_for_client(&scopes, &client_scopes) {
            return Err(CustomError::ValidationError(
                "Requested scopes exceed client's allowed scopes".to_string(),
            ));
        }

        // Generate token
        let token = generate_access_token();

        // Create the token in database
        let token_id = oauth_db::create_admin_access_token(
            &state.sqlx_pool,
            &token,
            client_id,
            user_id,
            &scopes,
            form.expires_in,
        )
        .await?;

        // Get the full token context for display
        let generated_token = oauth_db::get_oauth_token_by_id(&state.sqlx_pool, token_id)
            .await?
            .ok_or_else(|| CustomError::System("Failed to retrieve generated token".to_string()))?;

        web::render_template(
            state.template_env,
            "admin/oauth_tokens/generate.html",
            auth_session,
            context! {
                clients => oauth_db::list_oauth_clients(&state.sqlx_pool).await?,
                users => sqlx::query_as::<_, (i32, String, String, String)>(
                    r#"SELECT id, username, display_name, email FROM users WHERE active = true ORDER BY username"#
                )
                .fetch_all(&state.sqlx_pool)
                .await?,
                available_scopes => OAuthScope::all().iter().map(|s| s.as_str().to_string()).collect::<Vec<String>>(),
                form_data => Some(form),
                errors => None::<Option<minijinja::value::Value>>,
                generated_token => Some(generated_token),
            },
        )
        .await
    } else {
        // Process validation errors for template
        let client_id_errors: Vec<String> = validation_errors
            .field_errors()
            .get("client_id")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let user_id_errors: Vec<String> = validation_errors
            .field_errors()
            .get("user_id")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let scopes_errors: Vec<String> = validation_errors
            .field_errors()
            .get("scopes")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        let expires_in_errors: Vec<String> = validation_errors
            .field_errors()
            .get("expires_in")
            .map(|errors| {
                errors
                    .iter()
                    .map(|e| {
                        e.message
                            .as_ref()
                            .map(|m| m.to_string())
                            .unwrap_or_else(|| "unknown error".to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();

        web::render_template(
            state.template_env,
            "admin/oauth_tokens/generate.html",
            auth_session,
            context! {
                clients => oauth_db::list_oauth_clients(&state.sqlx_pool).await?,
                users => sqlx::query_as::<_, (i32, String, String, String)>(
                    r#"SELECT id, username, display_name, email FROM users WHERE active = true ORDER BY username"#
                )
                .fetch_all(&state.sqlx_pool)
                .await?,
                available_scopes => OAuthScope::all().iter().map(|s| s.as_str().to_string()).collect::<Vec<String>>(),
                form_data => Some(form),
                errors => Some(context! {
                    client_id => client_id_errors,
                    user_id => user_id_errors,
                    scopes => scopes_errors,
                    expires_in => expires_in_errors,
                }),
                generated_token => None::<AccessTokenWithContext>,
            },
        )
        .await
    }
}
