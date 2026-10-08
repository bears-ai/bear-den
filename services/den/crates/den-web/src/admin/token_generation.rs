//! Local validation and draft-preserving rendering for token issuance.

use super::{
    available_scope_names, context, field_error_messages, generate_access_token, oauth_db,
    oauth_feedback, scopes_from_json, selected_oauth_scopes, web, AppState, AuthSession,
    ClientOption, CustomError, GenerateTokenForm, IntoResponse, Redirect, Response, Session,
    TokenUserOption, TokenView, Validate, ValidationError, ValidationErrors,
};

pub(super) async fn render(
    state: &AppState,
    auth: AuthSession,
    form: Option<GenerateTokenForm>,
    errors: Option<ValidationErrors>,
    error: Option<&str>,
    generated_token: Option<TokenView>,
) -> Result<Response, CustomError> {
    let users =
        sqlx::query!(r"SELECT id, username, display_name, email FROM users ORDER BY username")
            .fetch_all(&state.sqlx_pool)
            .await?
            .into_iter()
            .map(|row| TokenUserOption::from((row.id, row.username, row.display_name, row.email)))
            .collect::<Vec<_>>();
    let errors = errors.map(|errors| {
        context! {
            client_id => field_error_messages(&errors, "client_id"),
            user_id => field_error_messages(&errors, "user_id"),
            scopes => field_error_messages(&errors, "scopes"),
            expires_in => field_error_messages(&errors, "expires_in"),
        }
    });
    web::render_template(
        state,
        "admin/oauth_tokens/generate.html",
        auth,
        context! {
            clients => oauth_db::list_oauth_clients(&state.sqlx_pool).await?
                            .into_iter().map(ClientOption::from).collect::<Vec<_>>(),
            users,
            available_scopes => available_scope_names(),
            form_data => form,
            errors,
            error,
            generated_token,
        },
    )
    .await
}

fn add_error(errors: &mut ValidationErrors, field: &'static str, message: &'static str) {
    errors.add(
        field,
        ValidationError::new("invalid_selection").with_message(message.into()),
    );
}

pub(super) async fn issue(
    state: &AppState,
    auth: AuthSession,
    session: Session,
    form: GenerateTokenForm,
) -> Result<Response, CustomError> {
    let mut errors = form.validate().err().unwrap_or_default();
    let client_id = form
        .client_id
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|id| *id > 0);
    let user_id = form.user_id.trim().parse::<i32>().ok().filter(|id| *id > 0);
    let client = match client_id {
        Some(id) => oauth_db::get_oauth_client_by_id(&state.sqlx_pool, id).await?,
        None => None,
    };
    if client.is_none() {
        add_error(
            &mut errors,
            "client_id",
            "Choose an existing active client.",
        );
    }
    let user_exists = match user_id {
        Some(user_id) => {
            sqlx::query_scalar!(
                r#"SELECT EXISTS(SELECT 1 FROM users WHERE id = $1) AS "exists!""#,
                user_id,
            )
            .fetch_one(&state.sqlx_pool)
            .await?
        }
        None => false,
    };
    if !user_exists {
        add_error(&mut errors, "user_id", "Choose an existing user.");
    }
    let scopes = selected_oauth_scopes(&form.scopes);
    if let Some(client) = &client {
        match scopes_from_json(&client.scopes) {
            Ok(allowed) if !den_oauth::oauth::utils::validate_scopes_for_client(&scopes, &allowed) => {
                add_error(&mut errors, "scopes", "Requested scopes exceed this client's allowed scopes. Change the scopes or client.");
            }
            Err(_) => add_error(&mut errors, "client_id", "This client's stored scopes are invalid. Repair the client before generating a token."),
            _ => {}
        }
    }
    if !errors.is_empty() {
        return render(state, auth, Some(form), Some(errors), None, None).await;
    }
    let token = generate_access_token();
    let issued = oauth_db::create_admin_access_token(
        &state.sqlx_pool,
        &token,
        client_id.expect("validated client"),
        user_id.expect("validated user"),
        &scopes,
        form.expires_in,
    )
    .await;
    let token_id = match issued {
        Ok(id) => id,
        Err(_) => return render(state, auth, Some(form), None,
            Some("Could not confirm that a token was created. Your selections are preserved; inspect the token list before retrying."), None).await,
    };
    if oauth_feedback::put_token(&session, &auth, token_id)
        .await
        .is_err()
    {
        return render(state, auth, Some(form), None,
            Some("The token was created, but one-time feedback could not be saved. Open the token list to inspect or revoke it; do not generate another token to recover this one."), None).await;
    }
    Ok(Redirect::to("/admin/oauth_tokens/generate").into_response())
}
