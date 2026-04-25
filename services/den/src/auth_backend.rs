// heavy borrowing from https://gitlab.com/maxhambraeus/axum-login-postgres-template

use std::collections::HashSet;

use async_trait::async_trait;
use axum_login::{AuthUser, AuthnBackend, AuthzBackend, UserId};
use password_auth::verify_password;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tokio::task;

use crate::core::user;
use crate::errors::CustomError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Database(#[from] CustomError),

    #[error(transparent)]
    TaskJoin(#[from] task::JoinError),

    #[error("User not found: {0}")]
    UserNotFound(String),
    #[error("Incorrect password")]
    BadCredentials(#[from] password_auth::VerifyError),
}

#[derive(Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub id: i32,
    pub username: String,
    passhash: String,
    pub is_admin: bool,
    pub theme: String,
}
// avoid logging the passhash
impl std::fmt::Debug for SessionUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("User")
            .field("id", &self.id)
            .field("username", &self.username)
            .field("passhash", &"[redacted]")
            .field("is_admin", &self.is_admin)
            .field("theme", &self.theme)
            .finish()
    }
}

impl AuthUser for SessionUser {
    type Id = i32;

    fn id(&self) -> Self::Id {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        self.passhash.as_bytes() // We use the password hash as the auth
        // hash--what this means
        // is when the user changes their password the
        // auth session becomes invalid.
    }
}

// This allows us to extract the authentication fields from forms. We use this
// to authenticate requests with the backend.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub next: Option<String>,
    pub su: bool,
}

#[derive(Debug, Clone)]
pub struct Backend {
    db: PgPool,
}

impl Backend {
    pub fn new(db: PgPool) -> Self {
        Self { db }
    }
}

#[async_trait]
impl AuthnBackend for Backend {
    type User = SessionUser;
    type Credentials = Credentials;
    type Error = Error;

    async fn authenticate(
        &self,
        creds: Self::Credentials,
    ) -> Result<Option<Self::User>, Self::Error> {
        // Check if the input looks like an email (contains '@')
        let db_user_result = if creds.username.contains('@') {
            user::db::get_user_auth_by_email(&self.db, &creds.username).await?
        } else {
            user::db::get_user_by_username(&self.db, &creds.username).await?
        };

        if let Some(db_user) = db_user_result {
            if creds.su {
                tracing::warn!("SU login to user: {}", creds.username);
                return Ok(Some(SessionUser {
                    id: db_user.id,
                    username: db_user.username,
                    passhash: db_user.passhash,
                    is_admin: db_user.admin_flag,
                    theme: db_user.theme,
                }));
            }

            match verify_password(creds.password, &db_user.passhash) {
                Ok(_) => Ok(Some(SessionUser {
                    id: db_user.id,
                    username: db_user.username,
                    passhash: db_user.passhash,
                    is_admin: db_user.admin_flag,
                    theme: db_user.theme,
                })),
                Err(e) => {
                    tracing::warn!(
                        "Authentication failed for user '{}': bad credentials",
                        creds.username
                    );
                    Err(Error::BadCredentials(e))
                }
            }
        } else {
            tracing::warn!(
                "Authentication failed for user '{}': user not found",
                creds.username
            );
            Err(Error::UserNotFound(format!("username: {}", creds.username)))
        }
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<Self::User>, Self::Error> {
        let db_user = user::db::get_user_by_id(&self.db, *user_id).await?;

        match db_user {
            None => Err(Error::UserNotFound(format!("id: {user_id}"))),
            Some(db_user) => {
                let user = Self::User {
                    id: db_user.id,
                    username: db_user.username,
                    passhash: db_user.passhash,
                    is_admin: db_user.admin_flag,
                    theme: db_user.theme,
                };

                Ok(Some(user))
            }
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct Permission {
    pub name: String,
}

impl From<&str> for Permission {
    fn from(name: &str) -> Self {
        Permission {
            name: name.to_string(),
        }
    }
}

#[async_trait]
impl AuthzBackend for Backend {
    type Permission = Permission;

    async fn get_user_permissions(
        &self,
        user: &Self::User,
    ) -> Result<HashSet<Self::Permission>, Self::Error> {
        let mut permissions = Vec::new();
        permissions.push(Permission::from(user.username.as_str()));
        if user.is_admin {
            permissions.push(Permission::from("admin"));
        }

        Ok(permissions.into_iter().collect())
    }

    // async fn get_group_permissions(
    //     &self,
    //     _user: &Self::User,
    // ) -> Result<HashSet<Self::Permission>, Self::Error> {
    //     // For now, no such thing as groups
    //     // eventually, maybe there are account tiers that grant permissions
    //     Ok(HashSet::new())
    // }

    // async fn get_all_permissions(
    //     &self,
    //     user: &Self::User,
    // ) -> Result<HashSet<Self::Permission>, Self::Error> {
    //     let mut all_perms = HashSet::new();
    //     all_perms.extend(self.get_user_permissions(user).await?);
    //     all_perms.extend(self.get_group_permissions(user).await?);
    //     Ok(all_perms)
    // }

    // async fn has_perm(
    //     &self,
    //     user: &Self::User,
    //     perm: Self::Permission,
    // ) -> Result<bool, Self::Error> {
    //     Ok(self.get_all_permissions(user).await?.contains(&perm))
    // }
}

// We use a type alias for convenience.
//
// Note that we've supplied our concrete backend here.
pub type AuthSession = axum_login::AuthSession<Backend>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_email_detection() {
        // Test that inputs with @ are detected as emails
        assert!("user@example.com".contains('@'));
        assert!("test.user@domain.co.uk".contains('@'));

        // Test that usernames without @ are not detected as emails
        assert!(!"username".contains('@'));
        assert!(!"user_name123".contains('@'));
        assert!(!"user-name".contains('@'));
    }

    #[test]
    fn test_credentials_struct() {
        let creds = Credentials {
            username: "testuser".to_string(),
            password: "password123".to_string(),
            next: None,
            su: false,
        };

        assert_eq!(creds.username, "testuser");
        assert!(!creds.username.contains('@'));

        let creds_with_email = Credentials {
            username: "user@example.com".to_string(),
            password: "password123".to_string(),
            next: None,
            su: false,
        };

        assert!(creds_with_email.username.contains('@'));
    }
}
