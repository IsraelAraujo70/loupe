use std::fmt;

use serde::{Deserialize, Serialize};

/// A repository, `owner/name`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoRef {
    pub owner: String,
    pub name: String,
}

impl RepoRef {
    pub fn new(owner: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            name: name.into(),
        }
    }

    /// Parses `owner/name`.
    pub fn parse(slug: &str) -> Option<Self> {
        let (owner, name) = slug.trim().split_once('/')?;
        if owner.is_empty() || name.is_empty() || name.contains('/') {
            return None;
        }
        Some(Self::new(owner, name))
    }

    pub fn url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }
}

impl fmt::Display for RepoRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// A pull request, `owner/name#number`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PrRef {
    pub repo: RepoRef,
    pub number: u64,
}

impl PrRef {
    pub fn new(repo: RepoRef, number: u64) -> Self {
        Self { repo, number }
    }

    /// Parses a pull request web URL such as
    /// `https://github.com/owner/name/pull/12` (extra path segments, a query,
    /// or a fragment are ignored) or the short form `owner/name#12`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some((slug, number)) = text.split_once('#')
            && !slug.contains(':')
        {
            return Some(Self::new(RepoRef::parse(slug)?, number.parse().ok()?));
        }
        let rest = text
            .strip_prefix("https://github.com/")
            .or_else(|| text.strip_prefix("http://github.com/"))
            .or_else(|| text.strip_prefix("github.com/"))?;
        let rest = rest.split(['?', '#']).next()?;
        let mut parts = rest.split('/');
        let owner = parts.next()?;
        let name = parts.next()?;
        if parts.next()? != "pull" {
            return None;
        }
        let number = parts.next()?.parse().ok()?;
        if owner.is_empty() || name.is_empty() {
            return None;
        }
        Some(Self::new(RepoRef::new(owner, name), number))
    }

    pub fn url(&self) -> String {
        format!("{}/pull/{}", self.repo.url(), self.number)
    }
}

impl fmt::Display for PrRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", self.repo, self.number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pull_request_urls_and_short_form() {
        let expected = PrRef::new(RepoRef::new("acme", "web"), 42);
        assert_eq!(
            PrRef::parse("https://github.com/acme/web/pull/42"),
            Some(expected.clone())
        );
        assert_eq!(
            PrRef::parse("https://github.com/acme/web/pull/42/files?w=1#diff"),
            Some(expected.clone())
        );
        assert_eq!(PrRef::parse("acme/web#42"), Some(expected.clone()));
        assert_eq!(expected.to_string(), "acme/web#42");
        assert_eq!(expected.url(), "https://github.com/acme/web/pull/42");
    }

    #[test]
    fn rejects_non_pull_request_urls() {
        assert_eq!(PrRef::parse("https://github.com/acme/web/issues/42"), None);
        assert_eq!(PrRef::parse("https://linear.app/acme/issue/ENG-1"), None);
        assert_eq!(PrRef::parse("acme#4"), None);
        assert_eq!(RepoRef::parse("acme/web/extra"), None);
    }
}
