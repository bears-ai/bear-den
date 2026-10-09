use den_core::tools::repository::RepositoryError;
use url::Url;

pub const GITHUB_API_HOST: &str = "api.github.com";

#[derive(Clone, PartialEq, Eq)]
pub struct GithubRepository {
    owner: String,
    repository: String,
    branch: Branch,
}

#[derive(Clone, PartialEq, Eq)]
struct Branch(String);

impl GithubRepository {
    pub fn parse(upstream: &str, configured_branch: &str) -> Result<Self, RepositoryError> {
        if upstream.len() > 1024 || upstream.contains(['%', '\\']) || !upstream.is_ascii() {
            return Err(RepositoryError::DestinationDenied);
        }
        let url = Url::parse(upstream).map_err(|_| RepositoryError::DestinationDenied)?;
        if url.scheme() != "https"
            || url.host_str() != Some("github.com")
            || url.port_or_known_default() != Some(443)
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.as_str() != upstream
        {
            return Err(RepositoryError::DestinationDenied);
        }
        let segments: Vec<_> = url.path().split('/').collect();
        if segments.len() != 3 || !segments[0].is_empty() {
            return Err(RepositoryError::DestinationDenied);
        }
        let owner = segments[1];
        let repository = segments[2].strip_suffix(".git").unwrap_or(segments[2]);
        if !component(owner, 100) || !component(repository, 100) {
            return Err(RepositoryError::DestinationDenied);
        }
        Ok(Self {
            owner: owner.to_owned(),
            repository: repository.to_owned(),
            branch: Branch::parse(configured_branch)?,
        })
    }

    pub fn owner(&self) -> &str {
        &self.owner
    }
    pub fn repository(&self) -> &str {
        &self.repository
    }
    pub fn branch(&self) -> &str {
        &self.branch.0
    }
    pub fn expected_ref(&self) -> String {
        format!("refs/heads/{}", self.branch.0)
    }

    pub fn api_url(&self) -> Url {
        let mut url = Url::parse("https://api.github.com/").expect("fixed GitHub URL");
        let mut segments = url.path_segments_mut().expect("fixed hierarchical URL");
        segments.extend([
            "repos",
            &self.owner,
            &self.repository,
            "git",
            "ref",
            "heads",
        ]);
        segments.extend(self.branch.0.split('/'));
        drop(segments);
        url
    }
}

fn component(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_' | b'.'))
}

impl Branch {
    fn parse(raw: &str) -> Result<Self, RepositoryError> {
        let value = raw.strip_prefix("refs/heads/").unwrap_or(raw);
        if value.is_empty()
            || value.len() > 255
            || value == "HEAD"
            || value.starts_with("refs/")
            || value.contains("..")
            || value.ends_with('.')
            || (value.len() == 40 && value.bytes().all(|ch| ch.is_ascii_hexdigit()))
            || value.split('/').any(|part| {
                !component(part, 255)
                    || part.starts_with(['.', '-'])
                    || part.ends_with('.')
                    || part.ends_with(".lock")
            })
        {
            return Err(RepositoryError::DestinationDenied);
        }
        Ok(Self(value.to_owned()))
    }
}

#[cfg(test)]
mod tests;
