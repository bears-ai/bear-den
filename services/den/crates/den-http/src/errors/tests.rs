use super::*;

fn page(error: CustomError) -> (StatusCode, String) {
    let response = error.into_response();
    let status = response.status();
    let bytes = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(axum::body::to_bytes(response.into_body(), 64 * 1024))
        .unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

#[test]
fn every_error_variant_omits_private_causes_and_preserves_status() {
    let private =
        "PRIVATE password=secret postgresql://user:password@db/private <script>alert(1)</script>";
    let cases = [
        (
            CustomError::Anyhow(anyhow::anyhow!(private).context("PRIVATE outer cause")),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            CustomError::System(private.into()),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            CustomError::Database(private.into()),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            CustomError::DatabaseUnavailable(private.into()),
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            CustomError::Session(private.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            CustomError::Authentication(private.into()),
            StatusCode::UNAUTHORIZED,
        ),
        (
            CustomError::Authorization(private.into()),
            StatusCode::FORBIDDEN,
        ),
        (
            CustomError::Parsing(private.into()),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            CustomError::Render(private.into()),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
        (
            CustomError::Email(private.into()),
            StatusCode::FAILED_DEPENDENCY,
        ),
        (CustomError::NotFound(private.into()), StatusCode::NOT_FOUND),
        (
            CustomError::ValidationError(private.into()),
            StatusCode::BAD_REQUEST,
        ),
    ];
    for (error, expected) in cases {
        let (status, html) = page(error);
        assert_eq!(status, expected);
        for secret in [
            "PRIVATE",
            "password=",
            "postgresql://",
            "<script>",
            "outer cause",
        ] {
            assert!(
                !html.contains(secret),
                "private cause must not cross HTTP boundary"
            );
        }
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("href=\"/\""));
        assert!(html.contains("check its current state"));
        assert!(html.contains(&format!("HTTP {}", status.as_u16())));
        assert!(!html.contains("<code>"));
        assert!(!html.contains("<details"));
        assert!(!html.contains("Please report"));
    }
}

#[test]
fn typed_error_categories_offer_safe_context_and_supported_recovery() {
    let (_, unavailable) = page(CustomError::DatabaseUnavailable("PRIVATE".into()));
    assert!(unavailable.contains("The service is temporarily unavailable."));
    assert!(unavailable.contains("Try opening the page again shortly."));
    let (_, auth) = page(CustomError::Authentication("PRIVATE".into()));
    assert!(auth.contains("You need to sign in to continue."));
    assert!(auth.contains("href=\"/login\""));
    let (_, forbidden) = page(CustomError::Authorization("PRIVATE".into()));
    assert!(forbidden.contains("You do not have access to this page or action."));
    assert!(!forbidden.contains("href=\"/login\""));
    let (_, validation) = page(CustomError::ValidationError("PRIVATE".into()));
    assert!(validation.contains("Return to the form, check your entries, and try again."));
    let (_, missing) = page(CustomError::NotFound("PRIVATE".into()));
    assert!(missing.contains("Page not found"));
    assert!(missing.contains("Check the address or return home."));
}

#[test]
fn diagnostic_display_is_retained_for_existing_server_logging_not_rendered_in_html() {
    let error = CustomError::Database("PRIVATE diagnostic".into());
    assert!(error.to_string().contains("PRIVATE diagnostic"));
    let (_, html) = page(error);
    assert!(!html.contains("PRIVATE diagnostic"));
    assert!(!html.contains("Reference:"));
    assert!(!html.contains("Hans"));
}
