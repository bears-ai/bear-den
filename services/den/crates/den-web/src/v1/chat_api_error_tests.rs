use super::access_tests::{app_with_runtime, bound_conversation, login, seed};
use super::chat_model_access_tests::{assert_json_error, raw_request};
use super::*;
use sqlx::PgPool;

const SECRET: &str = "postgres-password-api-token-CANARY";

#[sqlx::test(migrations = "../../migrations")]
async fn chat_model_and_conversation_rejections_are_json_with_request_references(pool: PgPool) {
    let (bear, [owner, other, _]) = seed(&pool).await;
    let canonical = bound_conversation(&pool, bear, owner, "conv-private-history").await;
    let app = app_with_runtime(
        &pool,
        crate::web_chat_runtime::unavailable_web_chat_runtime(),
    )
    .await;
    let cookie = login(&app, other).await;
    let owner_cookie = login(&app, owner).await;
    for route in ["model", "history", "notes", "artifacts"] {
        let body = assert_json_error(
            raw_request(
                &app,
                &cookie,
                "GET",
                &format!("/v1/chat/{route}?bear_id={bear}&conversation_id=conv-private-history"),
                "",
            )
            .await,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(body["code"], "access_unavailable");
    }
    let body = json!({"bear_id":bear, "title":SECRET}).to_string();
    let error = assert_json_error(
        raw_request(
            &app,
            &cookie,
            "PATCH",
            "/v1/chat/conversations/conv-private-history",
            &body,
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert!(!error.to_string().contains(SECRET));
    for route in ["model", "history", "notes", "artifacts", "conversations"] {
        let body = assert_json_error(
            raw_request(
                &app,
                &cookie,
                "GET",
                &format!("/v1/chat/{route}?bear_id={SECRET}"),
                "",
            )
            .await,
            StatusCode::BAD_REQUEST,
        )
        .await;
        assert_eq!(body["code"], "invalid_request");
        assert!(!body.to_string().contains(SECRET));
        let body = assert_json_error(
            raw_request(
                &app,
                &cookie,
                "GET",
                &format!(
                    "/v1/chat/{route}?bear_id={}&conversation_id=conv-private-history",
                    Uuid::new_v4()
                ),
                "",
            )
            .await,
            StatusCode::FORBIDDEN,
        )
        .await;
        assert_eq!(body["code"], "access_unavailable");
    }
    for (method, route) in [
        ("PATCH", "/v1/chat/model"),
        ("POST", "/v1/chat/conversations"),
        ("PATCH", "/v1/chat/conversations/conv-private-history"),
        ("POST", "/v1/chat/send"),
    ] {
        for (payload, status) in [
            (format!("{{invalid-{SECRET}"), StatusCode::BAD_REQUEST),
            (
                json!({"bear_id":SECRET}).to_string(),
                StatusCode::UNPROCESSABLE_ENTITY,
            ),
        ] {
            let body = assert_json_error(
                raw_request(&app, &owner_cookie, method, route, &payload).await,
                status,
            )
            .await;
            assert_eq!(body["code"], "invalid_request");
            assert!(!body.to_string().contains(SECRET));
        }
    }
    let bad_model = json!({"bear_id":bear, "conversation_id":"conv-private-history", "selection_mode":"explicit", "model":SECRET}).to_string();
    let body = assert_json_error(
        raw_request(&app, &owner_cookie, "PATCH", "/v1/chat/model", &bad_model).await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(!body.to_string().contains(SECRET));
    let body = assert_json_error(
        raw_request(
            &app,
            &owner_cookie,
            "GET",
            &format!("/v1/chat/model?bear_id={bear}&conversation_id={SECRET}"),
            "",
        )
        .await,
        StatusCode::BAD_REQUEST,
    )
    .await;
    assert!(!body.to_string().contains(SECRET));
    assert!(
        conversation_persistence::get_conversation_model_state(&pool, canonical)
            .await
            .unwrap()
            .is_none()
    );
    den_service::bears::db::revoke_membership(&pool, other, bear)
        .await
        .unwrap();
    let body = assert_json_error(
        raw_request(
            &app,
            &cookie,
            "GET",
            &format!("/v1/chat/conversations?bear_id={bear}"),
            "",
        )
        .await,
        StatusCode::FORBIDDEN,
    )
    .await;
    assert_eq!(body["code"], "access_unavailable");
}

#[tokio::test]
async fn model_api_boundary_never_renders_raw_service_causes() {
    for (error, status) in [
        (
            DenError::System(SECRET.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DenError::Database(SECRET.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DenError::DatabaseUnavailable(SECRET.into()),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            DenError::Session(SECRET.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DenError::Authentication(SECRET.into()),
            StatusCode::UNAUTHORIZED,
        ),
        (
            DenError::Authorization(SECRET.into()),
            StatusCode::FORBIDDEN,
        ),
        (DenError::NotFound(SECRET.into()), StatusCode::NOT_FOUND),
        (
            DenError::ValidationError(SECRET.into()),
            StatusCode::BAD_REQUEST,
        ),
        (DenError::Parsing(SECRET.into()), StatusCode::BAD_REQUEST),
        (
            DenError::Render(SECRET.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            DenError::Email(SECRET.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    ] {
        let response = ChatModelResponse::from_resolution(Err(error), None, vec![]);
        let response = match response {
            Err(error) => error.into_response(),
            Ok(model) => {
                // A typed validation failure can leave the model selector visible,
                // but cannot forward its cause or manufacture an effective model.
                assert!(model.effective_model.is_none());
                assert!(!serde_json::to_string(&model).unwrap().contains(SECRET));
                continue;
            }
        };
        let body = assert_json_error(response, status).await;
        assert!(!body.to_string().contains(SECRET));
    }
    for error in [
        CustomError::System(SECRET.into()),
        CustomError::Database(SECRET.into()),
    ] {
        let body = assert_json_error(
            ChatApiError::from(error).into_response(),
            StatusCode::INTERNAL_SERVER_ERROR,
        )
        .await;
        assert!(!body.to_string().contains(SECRET));
    }
    let request_id = Uuid::new_v4();
    let response = chat_send_error_response(CustomError::Database(SECRET.into()), request_id);
    let body = assert_json_error(response, StatusCode::INTERNAL_SERVER_ERROR).await;
    assert_eq!(body["request_id"], request_id.to_string());
    assert!(!body.to_string().contains(SECRET));
}
