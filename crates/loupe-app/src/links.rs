use loupe_github::PrRef;

/// What the pull request view knows about a PR, handed to the Linear and T3
/// panels so they can find related issues and threads, and start a review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrLinkContext {
    pub pr: PrRef,
    pub url: String,
    pub title: String,
    pub body: String,
    pub head_ref: String,
    pub head_oid: String,
    /// `owner/name` of the head when it lives on a fork; `None` when the
    /// head branch is in the pull request's own repository.
    pub head_fork: Option<String>,
    /// The author's login; `None` for a deleted account.
    pub author: Option<String>,
    /// The login of the GitHub account Loupe is signed in with.
    pub viewer_login: String,
    pub labels: Vec<String>,
}

impl PrLinkContext {
    /// Whether the signed-in account opened this pull request. GitHub logins
    /// ignore case.
    pub fn is_own(&self) -> bool {
        self.author
            .as_deref()
            .is_some_and(|author| author.eq_ignore_ascii_case(&self.viewer_login))
    }
}
