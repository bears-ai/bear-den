use crate::auth_backend;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use minijinja::context;

use std::fmt;

#[derive(Debug)]
pub enum CustomError {
    Anyhow(anyhow::Error),
    System(String),
    Database(String),
    Session(String),
    Authentication(String),
    Authorization(String),
    Render(String),
    Parsing(String),
    Email(String),
    NotFound(String),
    ValidationError(String),
}

impl std::error::Error for CustomError {}

// Allow the use of "{}" format specifier
impl fmt::Display for CustomError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            CustomError::Anyhow(ref cause) => {
                write!(f, "{cause:?}")
            }
            CustomError::System(ref cause) => {
                write!(f, "Server Error: {cause}")
            }
            CustomError::Database(ref cause) => {
                write!(f, "Database Error: {cause}")
            }
            CustomError::Session(ref cause) => {
                write!(f, "Session Error: {cause}")
            }
            CustomError::Authentication(ref cause) => {
                write!(f, "Authentication Error: {cause}")
            }
            CustomError::Authorization(ref cause) => {
                write!(f, "Authorization Error: {cause}")
            }
            CustomError::Render(ref cause) => {
                write!(f, "Rendering Error: {cause}")
            }
            CustomError::Parsing(ref cause) => {
                write!(f, "Parsing Error: {cause}")
            }
            CustomError::Email(ref cause) => {
                write!(f, "Email Error: {cause}")
            }
            CustomError::NotFound(ref cause) => write!(f, "Not Found: {cause}"),
            CustomError::ValidationError(ref cause) => {
                write!(f, "Validation Error: {cause}")
            }
        }
    }
}

impl IntoResponse for CustomError {
    fn into_response(self) -> Response {
        let error_string = self.to_string();
        let (error_name, error_message, status_code) = match self {
            CustomError::Anyhow(cause) => (
                "Server",
                format!("{cause:#}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            ),
            CustomError::System(message) => {
                ("Web server", message, StatusCode::UNPROCESSABLE_ENTITY)
            }
            CustomError::Database(message) => {
                ("Database", message, StatusCode::UNPROCESSABLE_ENTITY)
            }
            CustomError::Session(message) => {
                ("Session", message, StatusCode::INTERNAL_SERVER_ERROR)
            }
            CustomError::Authentication(message) => {
                ("Authentication", message, StatusCode::UNAUTHORIZED)
            }
            CustomError::Authorization(message) => {
                ("Authorization", message, StatusCode::FORBIDDEN)
            }
            CustomError::Parsing(message) => ("Parsing", message, StatusCode::UNPROCESSABLE_ENTITY),
            CustomError::Render(message) => {
                ("Rendering", message, StatusCode::INTERNAL_SERVER_ERROR)
            }
            CustomError::Email(message) => ("Email", message, StatusCode::FAILED_DEPENDENCY),
            CustomError::NotFound(message) => ("Not Found", message, StatusCode::NOT_FOUND),
            CustomError::ValidationError(message) => {
                ("Validation", message, StatusCode::BAD_REQUEST)
            }
        };

        tracing::error!("{}: {:#}", error_name, error_string);
        // discard any error to avoid recursion
        let mut template_env = minijinja::Environment::new();
        // let templates_dir = std::env::var("TEMPLATES_DIR").unwrap_or("src/web/templates".to_string());
        // minijinja_embed::embed_templates!(&templates_dir);
        #[cfg(feature = "production")]
        minijinja_embed::load_templates!(&mut template_env);
        #[cfg(not(feature = "production"))]
        template_env.set_loader(minijinja::path_loader("src/web/templates"));

        if let Ok(template) = template_env.get_template("error.html") {
            let body = template
                .render(context! {
                    error_code => status_code.as_u16(),
                    error_name,
                    error_message,
                })
                .unwrap();
            return (status_code, Html(body)).into_response();
        }
        (
            status_code,
            format!("Catastrophic error: [{error_name}] {error_message}"),
        )
            .into_response()
    }
}

impl From<anyhow::Error> for CustomError {
    fn from(err: anyhow::Error) -> CustomError {
        CustomError::Anyhow(err)
    }
}

impl From<std::io::Error> for CustomError {
    fn from(err: std::io::Error) -> CustomError {
        CustomError::System(err.to_string())
    }
}

impl From<axum::http::uri::InvalidUri> for CustomError {
    fn from(err: axum::http::uri::InvalidUri) -> CustomError {
        CustomError::System(err.to_string())
    }
}

impl From<sqlx::Error> for CustomError {
    fn from(err: sqlx::Error) -> CustomError {
        CustomError::Database(err.to_string())
    }
}

impl From<axum_login::tower_sessions::session::Error> for CustomError {
    fn from(err: axum_login::tower_sessions::session::Error) -> CustomError {
        CustomError::Session(err.to_string())
    }
}

impl From<auth_backend::Error> for CustomError {
    fn from(err: auth_backend::Error) -> CustomError {
        CustomError::Authentication(err.to_string())
    }
}

impl From<axum_login::Error<auth_backend::Backend>> for CustomError {
    fn from(err: axum_login::Error<auth_backend::Backend>) -> CustomError {
        CustomError::Authentication(err.to_string())
    }
}

impl From<serde_json::Error> for CustomError {
    fn from(err: serde_json::Error) -> CustomError {
        CustomError::Parsing(err.to_string())
    }
}

impl From<sqlx::types::uuid::Error> for CustomError {
    fn from(err: sqlx::types::uuid::Error) -> CustomError {
        CustomError::Parsing(err.to_string())
    }
}

impl From<mailgun_rs::SendError> for CustomError {
    fn from(err: mailgun_rs::SendError) -> CustomError {
        CustomError::Email(err.to_string())
    }
}

impl From<validator::ValidationErrors> for CustomError {
    fn from(err: validator::ValidationErrors) -> CustomError {
        CustomError::ValidationError(format!("{:?}", err))
    }
}
