//! The Inbox's filter: the command palette's `org:` and `repo:` prefixes
//! plus free text. It lives in [`Settings::notification_filter`] as one
//! token per term, so it survives restarts and the sidebar's count can
//! follow it.
//!
//! [`Settings::notification_filter`]: crate::store::Settings::notification_filter

use loupe_github::{Notification, RepoRef};

/// One term of the filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// The owner of the repository, whole and in any case, as GitHub's
    /// `org:` qualifier reads it.
    Org(String),
    /// `owner/name` names one repository; anything else is a part of a
    /// repository's full name.
    Repo(String),
    /// A word the title or the repository holds.
    Text(String),
}

impl Term {
    /// Reads one token: `org:x`, `repo:x` (or `repos:x`), or a word. A
    /// prefix with nothing after it is not a term yet.
    pub fn parse(token: &str) -> Option<Term> {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        let Some((prefix, value)) = token.split_once(':') else {
            return Some(Term::Text(token.to_owned()));
        };
        let term = match prefix.to_ascii_lowercase().as_str() {
            "org" => Term::Org,
            "repo" | "repos" => Term::Repo,
            _ => return Some(Term::Text(token.to_owned())),
        };
        (!value.is_empty()).then(|| term(value.to_owned()))
    }

    /// The token that reads back as this term.
    pub fn token(&self) -> String {
        match self {
            Term::Org(value) => format!("org:{value}"),
            Term::Repo(value) => format!("repo:{value}"),
            Term::Text(value) => value.clone(),
        }
    }

    /// The term that shows only `repo`.
    pub fn of_repo(repo: &RepoRef) -> Term {
        Term::Repo(repo.to_string())
    }
}

/// Every term in `query`, split on whitespace.
pub fn parse(query: &str) -> Vec<Term> {
    query.split_whitespace().filter_map(Term::parse).collect()
}

/// Splits what is typed in the field into the terms that are finished (a
/// space follows them) and the token still being typed.
pub fn finished(typed: &str) -> (Vec<Term>, String) {
    let ends_open = !typed.ends_with(char::is_whitespace);
    let mut tokens: Vec<&str> = typed.split_whitespace().collect();
    let rest = match ends_open {
        true => tokens.pop().unwrap_or_default().to_owned(),
        false => String::new(),
    };
    (tokens.into_iter().filter_map(Term::parse).collect(), rest)
}

/// The terms of the saved tokens.
pub fn from_tokens(tokens: &[String]) -> Vec<Term> {
    tokens
        .iter()
        .filter_map(|token| Term::parse(token))
        .collect()
}

/// Whether `notification` passes `terms`. Terms of one kind are
/// alternatives (`org:a org:b` shows both orgs); kinds narrow each other,
/// and every word must appear in the title or the repository.
pub fn matches(terms: &[Term], notification: &Notification) -> bool {
    let repo = &notification.repo;
    let full = repo.to_string().to_lowercase();
    let title = notification.title.to_lowercase();
    let (mut orgs, mut org_ok, mut repos, mut repo_ok) = (false, false, false, false);
    for term in terms {
        match term {
            Term::Org(org) => {
                orgs = true;
                org_ok |= org.eq_ignore_ascii_case(&repo.owner);
            }
            Term::Repo(value) => {
                let value = value.to_lowercase();
                repos = true;
                repo_ok |= match RepoRef::parse(&value) {
                    Some(_) => value == full,
                    None => full.contains(&value),
                };
            }
            Term::Text(word) => {
                let word = word.to_lowercase();
                if !title.contains(&word) && !full.contains(&word) {
                    return false;
                }
            }
        }
    }
    (!orgs || org_ok) && (!repos || repo_ok)
}

/// The notifications of `list` that pass `terms`.
pub fn apply<'a>(terms: &[Term], list: &'a [Notification]) -> Vec<&'a Notification> {
    list.iter()
        .filter(|notification| matches(terms, notification))
        .collect()
}

/// `tokens` with `added` after them, each term once.
pub fn with_terms(tokens: &[String], added: &[Term]) -> Vec<String> {
    let mut terms = from_tokens(tokens);
    for term in added {
        if !terms.contains(term) {
            terms.push(term.clone());
        }
    }
    terms.iter().map(Term::token).collect()
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use loupe_github::{Notification, NotificationSubject, PrRef, RepoRef};

    use super::{Term, apply, finished, from_tokens, matches, parse, with_terms};

    fn notification(repo: &str, title: &str) -> Notification {
        let repo = RepoRef::parse(repo).expect("repo");
        Notification {
            id: format!("{repo} {title}"),
            repo: repo.clone(),
            title: title.into(),
            subject: NotificationSubject::PullRequest(PrRef::new(repo, 1)),
            reason: "mention".into(),
            unread: true,
            updated_at: DateTime::from_timestamp(0, 0).expect("timestamp"),
        }
    }

    fn shown(query: &str, list: &[Notification]) -> Vec<String> {
        apply(&parse(query), list)
            .into_iter()
            .map(|n| n.id.clone())
            .collect()
    }

    #[test]
    fn prefixes_become_terms_and_the_rest_is_text() {
        assert_eq!(
            parse("org:hooli-inc  repos:loupe REPO:a/b fix org: other:x"),
            [
                Term::Org("hooli-inc".into()),
                Term::Repo("loupe".into()),
                Term::Repo("a/b".into()),
                Term::Text("fix".into()),
                Term::Text("other:x".into()),
            ]
        );
        for term in parse("org:a repo:b c") {
            assert_eq!(Term::parse(&term.token()), Some(term));
        }
    }

    #[test]
    fn org_keeps_only_that_owner() {
        let list = [
            notification("IsraelAraujo70/loupe", "CI failed"),
            notification("hooli-inc/hooli-api", "Review requested"),
            notification("israelaraujo70x/other", "Mention"),
        ];
        assert_eq!(
            shown("org:israelaraujo70", &list),
            ["IsraelAraujo70/loupe CI failed"]
        );
        assert_eq!(shown("org:IsraelAraujo70 org:hooli-inc", &list).len(), 2);
    }

    #[test]
    fn repo_names_one_repository_or_a_part_of_one() {
        let list = [
            notification("IsraelAraujo70/loupe", "a"),
            notification("IsraelAraujo70/loupe-old", "b"),
            notification("hooli-inc/hooli-api", "c"),
        ];
        assert_eq!(
            shown("repo:israelaraujo70/loupe", &list),
            ["IsraelAraujo70/loupe a"]
        );
        assert_eq!(shown("repo:loup", &list).len(), 2);
        assert_eq!(shown("repo:hooli-api repo:pe-old", &list).len(), 2);
    }

    #[test]
    fn kinds_narrow_each_other_and_words_search_title_and_repository() {
        let list = [
            notification("hooli-inc/hooli-api", "fix(auth): quotas"),
            notification("hooli-inc/hooli-web", "feat: login"),
            notification("IsraelAraujo70/prism", "fix palette"),
        ];
        assert_eq!(
            shown("org:hooli-inc FIX", &list),
            ["hooli-inc/hooli-api fix(auth): quotas"]
        );
        assert_eq!(shown("org:hooli-inc repo:prism", &list).len(), 0);
        assert_eq!(shown("web", &list), ["hooli-inc/hooli-web feat: login"]);
        assert!(matches(&[], &list[0]), "no terms shows everything");
    }

    #[test]
    fn a_space_finishes_a_term_and_the_last_word_stays_in_the_field() {
        assert_eq!(finished("org:a"), (Vec::new(), "org:a".to_owned()));
        assert_eq!(
            finished("org:a "),
            (vec![Term::Org("a".into())], String::new())
        );
        assert_eq!(
            finished("repo:x fix"),
            (vec![Term::Repo("x".into())], "fix".to_owned())
        );
        assert_eq!(finished("org: "), (Vec::new(), String::new()));
        assert_eq!(finished(""), (Vec::new(), String::new()));
    }

    #[test]
    fn adding_a_term_twice_keeps_one() {
        let tokens = with_terms(&["org:a".into()], &parse("repo:x/y org:a"));
        assert_eq!(tokens, ["org:a", "repo:x/y"]);
        assert_eq!(from_tokens(&tokens).len(), 2);
    }
}
