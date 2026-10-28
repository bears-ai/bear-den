use sqlx::PgPool;

use crate::oauth::{
    db, endpoints::AuthorizationRequestWithPKCE, jwt::create_jwt_manager, utils, OAuthClient,
    OAuthScope, TokenRequest,
};

pub(crate) const CALLBACK: &str = "com.bears.tests:/oauth/callback";
pub(crate) const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

pub(crate) struct Fixture {
    pub user_id: i32,
    pub client: OAuthClient,
    pub scopes: Vec<OAuthScope>,
}

pub(crate) async fn fixture(pool: &PgPool, public: bool, scopes: Vec<OAuthScope>) -> Fixture {
    let suffix = utils::generate_secure_random_string(16);
    let user_id = den_http::user::db::create_user(
        pool,
        &format!("{suffix}@example.test"),
        &suffix,
        "Native human",
        "test-password-hash",
    )
    .await
    .unwrap();
    let identifier = utils::generate_client_id();
    let hash = (!public).then(|| utils::hash_client_secret("test-client-secret").unwrap());
    db::create_oauth_client(
        pool,
        &identifier,
        hash.as_deref(),
        "Native tests",
        &serde_json::json!([CALLBACK]),
        &scopes,
        public,
    )
    .await
    .unwrap();
    let client = db::get_oauth_client_by_client_id(pool, &identifier)
        .await
        .unwrap()
        .unwrap();
    Fixture {
        user_id,
        client,
        scopes,
    }
}

impl Fixture {
    pub fn authorization(&self) -> AuthorizationRequestWithPKCE {
        AuthorizationRequestWithPKCE {
            response_type: "code".into(),
            client_id: self.client.client_id.clone(),
            redirect_uri: CALLBACK.into(),
            scope: Some(crate::oauth::scopes_to_string(&self.scopes)),
            state: Some("native-state".into()),
            code_challenge: Some(utils::generate_pkce_code_challenge(VERIFIER)),
            code_challenge_method: Some("S256".into()),
        }
    }

    pub async fn code(&self, pool: &PgPool) -> TokenRequest {
        let authorization = self.authorization();
        let code = utils::generate_authorization_code();
        db::create_authorization_code(
            pool,
            &code,
            self.client.id,
            self.user_id,
            CALLBACK,
            &self.scopes,
            utils::authorization_code_expiration(),
            authorization.code_challenge.as_deref(),
            Some("S256"),
        )
        .await
        .unwrap();
        TokenRequest {
            grant_type: "authorization_code".into(),
            code,
            redirect_uri: CALLBACK.into(),
            client_id: self.client.client_id.clone(),
            client_secret: (!self.client.public).then(|| "test-client-secret".into()),
            code_verifier: Some(VERIFIER.into()),
        }
    }

    pub async fn token(&self, pool: &PgPool) -> String {
        let expires_at = utils::access_token_expiration();
        let token = create_jwt_manager()
            .generate_access_token(
                self.user_id,
                &self.client.client_id,
                &self.scopes,
                expires_at,
            )
            .unwrap();
        db::create_access_token(
            pool,
            &token,
            self.client.id,
            self.user_id,
            &self.scopes,
            expires_at,
        )
        .await
        .unwrap();
        token
    }
}
