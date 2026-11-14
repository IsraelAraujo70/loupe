use loupe_github::{PrRef, PullRequestDetail};

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
    /// What GitHub's detail of a pull request says about it.
    pub fn from_detail(detail: &PullRequestDetail) -> Self {
        PrLinkContext {
            pr: detail.pr.clone(),
            url: detail.url.clone(),
            title: detail.title.clone(),
            body: detail.body.clone(),
            head_ref: detail.head_ref.clone(),
            head_oid: detail.head_oid.clone(),
            head_fork: detail.head_repo.as_ref().map(ToString::to_string),
            author: detail.author.as_ref().map(|author| author.login.clone()),
            viewer_login: detail.viewer_login.clone(),
            labels: detail
                .labels
                .iter()
                .map(|label| label.name.clone())
                .collect(),
        }
    }

    /// Whether the signed-in account opened this pull request. GitHub logins
    /// ignore case.
    pub fn is_own(&self) -> bool {
        self.author
            .as_deref()
            .is_some_and(|author| author.eq_ignore_ascii_case(&self.viewer_login))
    }
}
