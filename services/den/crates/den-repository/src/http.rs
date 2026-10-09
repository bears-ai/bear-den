use den_core::tools::{
    repository::{CommitSha, RepositoryError},
    support::is_public_ip,
};
use reqwest::{
    header::{HeaderValue, AUTHORIZATION},
    Client,
};
use secrecy::ExposeSecret;
use serde::Deserialize;
use std::{net::SocketAddr, time::Duration};
use url::Url;
use zeroize::Zeroizing;

use crate::{CredentialLease, GithubRepository, GITHUB_API_HOST};

const BODY_LIMIT: usize = 16 * 1024;

pub(crate) enum Transport {
    Public,
    #[cfg(any(test, feature = "test-util"))]
    Loopback(SocketAddr),
}

pub(crate) struct Prepared {
    client: Client,
    url: Url,
}

impl Transport {
    pub(crate) async fn prepare(
        self,
        repository: &GithubRepository,
    ) -> Result<Prepared, RepositoryError> {
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy();
        #[cfg(not(any(test, feature = "test-util")))]
        let url = repository.api_url();
        #[cfg(any(test, feature = "test-util"))]
        let mut url = repository.api_url();
        match self {
            Self::Public => {
                let addresses: Vec<_> = tokio::net::lookup_host((GITHUB_API_HOST, 443))
                    .await
                    .map_err(|_| RepositoryError::DestinationDenied)?
                    .collect();
                validate_addresses(&addresses)?;
                builder = builder.resolve_to_addrs(GITHUB_API_HOST, &addresses);
            }
            #[cfg(any(test, feature = "test-util"))]
            Self::Loopback(address) => {
                assert!(address.ip().is_loopback());
                url.set_scheme("http").expect("test scheme");
                url.set_host(Some(&address.ip().to_string()))
                    .expect("test host");
                url.set_port(Some(address.port())).expect("test port");
            }
        }
        Ok(Prepared {
            client: builder
                .build()
                .map_err(|_| RepositoryError::ProviderUnavailable)?,
            url,
        })
    }
}

fn validate_addresses(addresses: &[SocketAddr]) -> Result<(), RepositoryError> {
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|address| address.port() != 443 || !is_public_ip(address.ip()))
    {
        return Err(RepositoryError::DestinationDenied);
    }
    Ok(())
}

impl Prepared {
    pub(crate) async fn send(
        self,
        repository: &GithubRepository,
        lease: &CredentialLease,
    ) -> Result<CommitSha, RepositoryError> {
        // This edge is the only place secret bytes are exposed. reqwest/header/TLS
        // copies are not zeroizing; sensitive marking prevents accidental Debug output.
        let token = lease.token.expose_secret();
        if token.is_empty()
            || token.len() > 4096
            || !token
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'_' | b'-'))
        {
            return Err(RepositoryError::CredentialScopeMismatch);
        }
        let header_bytes = Zeroizing::new(format!("Bearer {token}"));
        let mut header = HeaderValue::from_str(&header_bytes)
            .map_err(|_| RepositoryError::CredentialScopeMismatch)?;
        header.set_sensitive(true);
        let mut response = self
            .client
            .get(self.url)
            .header(AUTHORIZATION, header)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, "bear-den-repository")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(transport_error)?;
        for value in response.headers_mut().values_mut() {
            value.set_sensitive(true);
        }
        match response.status().as_u16() {
            200 => {}
            301..=399 => return Err(RepositoryError::DestinationDenied),
            401 | 403 => return Err(RepositoryError::ProviderAuthenticationFailed),
            429 => return Err(RepositoryError::RateLimited),
            _ => return Err(RepositoryError::ProviderUnavailable),
        }
        if response
            .content_length()
            .is_some_and(|length| length > BODY_LIMIT as u64)
        {
            return Err(RepositoryError::ResponseTooLarge);
        }
        let mut bytes = Zeroizing::new(Vec::with_capacity(BODY_LIMIT));
        while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
            if bytes.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return Err(RepositoryError::ResponseTooLarge);
            }
            bytes.extend_from_slice(&chunk);
        }
        decode(&bytes, repository, token)
    }
}

fn transport_error(error: reqwest::Error) -> RepositoryError {
    if error.is_timeout() {
        RepositoryError::Timeout
    } else {
        RepositoryError::ProviderUnavailable
    }
}

#[derive(Deserialize)]
struct RefResponse<'a> {
    #[serde(borrow, rename = "ref")]
    reference: &'a str,
    #[serde(borrow)]
    object: CommitObject<'a>,
}
#[derive(Deserialize)]
struct CommitObject<'a> {
    #[serde(borrow, rename = "type")]
    kind: &'a str,
    #[serde(borrow)]
    sha: &'a str,
}

fn decode(
    bytes: &[u8],
    repository: &GithubRepository,
    token: &str,
) -> Result<CommitSha, RepositoryError> {
    let response: RefResponse<'_> =
        serde_json::from_slice(bytes).map_err(|_| RepositoryError::InvalidProviderResponse)?;
    let sha = response.object.sha;
    if response.reference != repository.expected_ref() || response.object.kind != "commit"
        || sha.len() != 40 || token.is_empty()
        // Even case-normalized echoes or credential fragments must not become a SHA.
        || sha.as_bytes().windows(token.len()).any(|part| part.eq_ignore_ascii_case(token.as_bytes()))
        || token.as_bytes().windows(sha.len()).any(|part| part.eq_ignore_ascii_case(sha.as_bytes()))
    {
        return Err(RepositoryError::InvalidProviderResponse);
    }
    CommitSha::parse(response.object.sha)
}

#[cfg(test)]
mod tests;
