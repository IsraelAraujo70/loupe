//! "Review with T3 Code": whether a pull request can get a review thread,
//! which reviewer it gets, and the request the thread starts with, from the
//! template and the risk rules of Settings > T3 Code. Pure, so every rule
//! here is tested without a server.

use std::collections::HashMap;

use loupe_t3::{ModelChoice, Project, PullRequestKey, ReviewFacts, Session};
use serde::{Deserialize, Serialize};

use crate::links::PrLinkContext;

/// Why the button is disabled when the token cannot start agents. Settings
/// shows the same sentence next to the token.
pub const OPERATE_HINT: &str = "Review with T3 Code starts an agent, so it needs a T3 token \
     with full access (orchestration:operate). A read-only token still lists the threads.";

pub(super) const CONNECT_HINT: &str = "Connect T3 Code in Settings to start a review.";
pub(super) const REJECTED_HINT: &str = "T3 rejected the token. Check it in Settings.";

const OPUS: &str = "claude-opus-5-5";
const FABLE: &str = "claude-fable-5-2";
/// The models a review runs on, for `T3::review_facts`.
pub(super) const MODELS: [&str; 2] = [OPUS, FABLE];

/// Who reviews: the general review, or the general review plus the security
/// review of the `revisar-seguranca` skill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reviewer {
    General,
    GeneralAndSecurity,
}

impl Reviewer {
    pub(super) const ALL: [Reviewer; 2] = [Reviewer::General, Reviewer::GeneralAndSecurity];

    pub(super) fn label(self) -> &'static str {
        match self {
            Reviewer::General => "General (Opus)",
            Reviewer::GeneralAndSecurity => "General + security (Fable)",
        }
    }

    fn model(self) -> (&'static str, &'static str) {
        match self {
            Reviewer::General => (OPUS, "high"),
            Reviewer::GeneralAndSecurity => (FABLE, "medium"),
        }
    }
}

/// Where a review of this pull request would run, once every check passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Plan {
    pub project_id: String,
    pub project_title: String,
    /// The worktree's new branch, one no thread of the project holds.
    pub branch: String,
    /// The provider instance for each reviewer's model.
    instances: HashMap<&'static str, String>,
    /// Why the pull request is risky: its risk labels and the changed files
    /// that touch auth, secrets, permissions, money, uploads or personal
    /// data. Empty for an ordinary pull request.
    pub risk: Vec<String>,
}

impl Plan {
    /// The reviewer the dialog starts with: security for a risky pull request.
    pub(super) fn default_reviewer(&self) -> Reviewer {
        if self.risk.is_empty() {
            Reviewer::General
        } else {
            Reviewer::GeneralAndSecurity
        }
    }

    pub(super) fn model(&self, reviewer: Reviewer) -> ModelChoice {
        let (model, effort) = reviewer.model();
        ModelChoice {
            instance_id: self.instances[model].clone(),
            model: model.to_owned(),
            effort: Some(effort.to_owned()),
        }
    }
}

/// The state of the button on a pull request someone else opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Readiness {
    Checking,
    /// Disabled, with the reason as its tooltip.
    Unavailable(String),
    Ready(Plan),
}

/// Whether the pull request can get a review thread, from what the server and
/// GitHub answered. `files` are the changed paths.
pub(super) fn assess(
    context: &PrLinkContext,
    session: Session,
    projects: &[Project],
    facts: &ReviewFacts,
    files: &[String],
    settings: &ReviewSettings,
) -> Readiness {
    if !session.can_operate {
        return Readiness::Unavailable(OPERATE_HINT.to_owned());
    }
    let repository = context.pr.repo.to_string();
    let key = PullRequestKey::github(repository.clone(), context.pr.number);
    let Some(project) = projects.iter().find(|project| project.holds(&key)) else {
        return Readiness::Unavailable(format!(
            "No T3 project tracks {repository}. Add the repository to T3 Code first."
        ));
    };
    // Both models are Claude's, so one provider instance serves the other
    // when only one of them has run yet.
    let found = |model: &str| {
        facts
            .instances
            .get(model)
            .filter(|id| !id.is_empty())
            .cloned()
    };
    let Some(any) = found(OPUS).or_else(|| found(FABLE)) else {
        return Readiness::Unavailable(
            "No T3 thread has run Claude Opus or Fable yet, so Loupe cannot tell which \
             provider serves them."
                .to_owned(),
        );
    };
    let instances = [OPUS, FABLE]
        .into_iter()
        .map(|model| (model, found(model).unwrap_or_else(|| any.clone())))
        .collect();
    let taken = facts
        .branches
        .get(&project.id)
        .map(Vec::as_slice)
        .unwrap_or_default();
    Readiness::Ready(Plan {
        project_id: project.id.clone(),
        project_title: project.title.clone(),
        branch: branch(context, taken),
        instances,
        risk: risk(settings, &context.labels, files),
    })
}

/// A disabled state that needs no server: the head lives on a fork, which a
/// worktree started from the repository's own remote cannot check out.
pub(super) fn precheck(context: &PrLinkContext) -> Option<Readiness> {
    context.head_fork.as_ref().map(|fork| {
        Readiness::Unavailable(format!(
            "The head branch is on the fork {fork}, and T3 starts the review from {}.",
            context.pr.repo
        ))
    })
}

/// The review settings of Settings > T3 Code: the request template and the
/// rules that mark a pull request risky. `None` is Loupe's default, so a user
/// who never edited a setting gets the default of each new version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReviewSettings {
    /// The request template, with the `{variables}` of [`VARIABLES`].
    pub prompt: Option<String>,
    /// Off: no pull request is risky, and every review starts on the
    /// general reviewer.
    pub risk_check: bool,
    pub risk_labels: Option<Vec<String>>,
    pub risk_words: Option<Vec<String>>,
    pub risk_exceptions: Option<Vec<String>>,
}

impl Default for ReviewSettings {
    fn default() -> Self {
        ReviewSettings {
            prompt: None,
            risk_check: true,
            risk_labels: None,
            risk_words: None,
            risk_exceptions: None,
        }
    }
}

impl ReviewSettings {
    /// The template the dialog fills.
    pub fn prompt(&self) -> &str {
        self.prompt.as_deref().unwrap_or(DEFAULT_PROMPT)
    }

    /// Keeps `text` as the template; the default text is kept as the
    /// default, so it follows Loupe's updates.
    pub fn set_prompt(&mut self, text: &str) {
        self.prompt = (text != DEFAULT_PROMPT).then(|| text.to_owned());
    }

    /// The words of one risk list, the user's or the default.
    pub fn list(&self, list: RiskList) -> Vec<String> {
        match list.slot(self) {
            Some(words) => words.clone(),
            None => list
                .defaults()
                .iter()
                .map(|word| (*word).to_owned())
                .collect(),
        }
    }

    /// Keeps the words of `text` (see [`parse_list`]) as the list; the
    /// default words are kept as the default.
    pub fn set_list(&mut self, list: RiskList, text: &str) {
        let words = parse_list(text);
        let default = words
            .iter()
            .map(String::as_str)
            .eq(list.defaults().iter().copied());
        *list.slot_mut(self) = (!default).then_some(words);
    }
}

/// One of the editable lists of the risk check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RiskList {
    /// A label that contains one of these marks the pull request risky.
    Labels,
    /// A changed file with a path word that starts with one of these.
    Words,
    /// Path words that start like a risk word and are not one.
    Exceptions,
}

impl RiskList {
    fn defaults(self) -> &'static [&'static str] {
        match self {
            RiskList::Labels => &RISK_LABELS,
            RiskList::Words => &RISK_WORDS,
            RiskList::Exceptions => &NOT_RISK,
        }
    }

    fn slot(self, settings: &ReviewSettings) -> &Option<Vec<String>> {
        match self {
            RiskList::Labels => &settings.risk_labels,
            RiskList::Words => &settings.risk_words,
            RiskList::Exceptions => &settings.risk_exceptions,
        }
    }

    fn slot_mut(self, settings: &mut ReviewSettings) -> &mut Option<Vec<String>> {
        match self {
            RiskList::Labels => &mut settings.risk_labels,
            RiskList::Words => &mut settings.risk_words,
            RiskList::Exceptions => &mut settings.risk_exceptions,
        }
    }
}

/// The entries of a list as Settings shows it: separated by commas or
/// lines, trimmed, without empty entries or repeats (in any case). A label
/// may hold spaces, so spaces do not separate.
pub fn parse_list(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in text.split([',', '\n']).map(str::trim) {
        if !word.is_empty() && !words.iter().any(|known| known.eq_ignore_ascii_case(word)) {
            words.push(word.to_owned());
        }
    }
    words
}

/// A list as Settings shows it, for [`parse_list`] to read back.
pub fn format_list(words: &[String]) -> String {
    words.join(", ")
}

const RISK_LABELS: [&str; 5] = ["risk", "risco", "security", "seguranca", "segurança"];

/// Words of a path that mark auth, secrets, permissions, money, uploads or
/// personal data. A path word matches when it starts with one of them.
const RISK_WORDS: [&str; 35] = [
    "auth",
    "login",
    "logout",
    "session",
    "token",
    "oauth",
    "jwt",
    "password",
    "passwd",
    "secret",
    "credential",
    "cert",
    "crypt",
    "permission",
    "role",
    "rbac",
    "acl",
    "polic",
    "guard",
    "payment",
    "billing",
    "invoice",
    "money",
    "wallet",
    "pix",
    "boleto",
    "ledger",
    "transfer",
    "refund",
    "upload",
    "pii",
    "personal",
    "privacy",
    "cpf",
    "lgpd",
];

/// Words that start like a risk word and are not one.
const NOT_RISK: [&str; 4] = ["author", "certain", "pixel", "tokeniz"];

/// Why a pull request is risky under `settings`: each risk label, then each
/// changed file that touches a sensitive area. Empty when nothing does or
/// the risk check is off.
pub(super) fn risk(settings: &ReviewSettings, labels: &[String], files: &[String]) -> Vec<String> {
    if !settings.risk_check {
        return Vec::new();
    }
    let lowercase = |list: RiskList| -> Vec<String> {
        settings
            .list(list)
            .iter()
            .map(|word| word.trim().to_lowercase())
            .filter(|word| !word.is_empty())
            .collect()
    };
    let (risk_labels, risk_words, not_risk) = (
        lowercase(RiskList::Labels),
        lowercase(RiskList::Words),
        lowercase(RiskList::Exceptions),
    );
    let labels = labels.iter().filter(|label| {
        let label = label.to_lowercase();
        risk_labels.iter().any(|word| label.contains(word.as_str()))
    });
    let files = files.iter().filter(|path| {
        words(path).iter().any(|word| {
            !not_risk.iter().any(|not| word.starts_with(not.as_str()))
                && risk_words
                    .iter()
                    .any(|risk| word.starts_with(risk.as_str()))
        })
    });
    labels
        .map(|label| format!("label {label}"))
        .chain(files.cloned())
        .collect()
}

/// The lowercase words of a path, split at punctuation, digits, and camel
/// case: `src/PaymentService.ts` gives `src`, `payment`, `service`, `ts`.
fn words(path: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;
    for ch in path.chars() {
        let starts_word = ch.is_uppercase() && previous_lower;
        if (!ch.is_alphabetic() || starts_word) && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        if ch.is_alphabetic() {
            current.extend(ch.to_lowercase());
        }
        previous_lower = ch.is_lowercase();
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// The new thread's title, as T3's sidebar lists it.
pub(super) fn thread_title(context: &PrLinkContext) -> String {
    let author = context.author.as_deref().unwrap_or("ghost");
    let title: String = context.title.chars().take(90).collect();
    format!("Revisar PR #{} de {author}: {title}", context.pr.number)
}

/// The worktree's branch: `review/pr-<n>`, or `review/pr-<n>-2` and on
/// when a thread of the project holds the name: git refuses to create a
/// branch twice, and T3 keeps the branch of an earlier review.
fn branch(context: &PrLinkContext, taken: &[String]) -> String {
    let base = format!("review/pr-{}", context.pr.number);
    std::iter::once(base.clone())
        .chain((2..).map(|k| format!("{base}-{k}")))
        .find(|name| !taken.iter().any(|branch| branch == name))
        .unwrap_or(base)
}

/// The template a review request starts from until the user edits it in
/// Settings. With it, Loupe sends the request it sent before the template
/// was a setting (pinned by the `golden` tests).
pub const DEFAULT_PROMPT: &str = "Revisar o PR #{number} de {repository}, de `{author}`: \"{title}\".
{url}

Esta worktree parte do head atual de `origin/{head_branch}` (era `{short_sha}` quando o pedido foi escrito).

1. Leia o AGENTS.md e o CLAUDE.md do repo, a descrição do PR (`gh pr view {number} -R {repository}`), os comentários e as revisões já feitas (`gh api repos/{repository}/pulls/{number}/comments`, `gh api repos/{repository}/issues/{number}/comments`, `gh api repos/{repository}/pulls/{number}/reviews`).
2. Use os revisores versionados do repo (`.claude/agents/`) e as skills `verify*` do repo como checklist, quando existirem.
3. **Resolve o que diz resolver?** Confira contra o ticket ligado no Linear, quando houver.
4. **Achados**, cada um com arquivo:linha, gravidade (🔴 bloqueia / 🟡 recomendação) e uma prova: um teste, uma reprodução ou o caminho no código.
5. **Testes**: rode os specs que o PR toca (os pesados onde o AGENTS.md do repo mandar) e mostre que uma mutação relevante faz um teste falhar.
6. **Escopo**: diga o que vazou para fora do que o PR pede.

{security}

Não comente no PR, não aprove, não faça push nem merge. Termine com um texto pronto para o usuário postar como revisão, o veredito (aprovar, aprovar com ressalvas ou pedir mudanças) e um bloco json: {\"veredito\":\"...\",\"vermelhos\":<n>,\"amarelos\":<n>,\"nao_verificado\":\"...\"}.";

/// The variables a template can use, each with what it stands for.
pub const VARIABLES: [(&str, &str); 9] = [
    ("number", "the pull request's number"),
    ("title", "its title"),
    ("author", "its author's login"),
    ("url", "its URL"),
    ("repository", "owner/name"),
    ("head_branch", "the head branch"),
    ("head_sha", "the head commit's SHA"),
    ("short_sha", "its first 8 characters"),
    (
        "security",
        "the security review request, when the reviewer includes it",
    ),
];

/// The review request the thread starts with: `template` with its
/// variables filled for this pull request and reviewer. The user can edit
/// it before sending.
pub(super) fn request(
    context: &PrLinkContext,
    reviewer: Reviewer,
    risk: &[String],
    template: &str,
) -> String {
    let security = match reviewer {
        Reviewer::General => String::new(),
        Reviewer::GeneralAndSecurity => {
            let why = if risk.is_empty() {
                "O usuário pediu também a revisão de segurança".to_owned()
            } else {
                format!("Este PR é de risco ({})", summary(risk))
            };
            format!(
                "{why}: além da revisão geral, faça a revisão de segurança seguindo a skill \
                 `revisar-seguranca`, quando o ambiente tiver essa skill."
            )
        }
    };
    let value = |name: &str| -> Option<String> {
        Some(match name {
            "number" => context.pr.number.to_string(),
            "title" => context.title.clone(),
            "author" => context.author.clone().unwrap_or_else(|| "ghost".to_owned()),
            "url" => context.url.clone(),
            "repository" => context.pr.repo.to_string(),
            "head_branch" => context.head_ref.clone(),
            "head_sha" => context.head_oid.clone(),
            "short_sha" => context.head_oid.chars().take(8).collect(),
            "security" => security.clone(),
            _ => return None,
        })
    };
    fill(template, value)
}

/// `template` with each `{variable}` that `value` knows replaced in one
/// pass, so a value that holds braces stays as it is. A paragraph that is
/// only a variable with an empty value (`{security}` for the general
/// review) is left out with its blank line. Other text in braces, such as
/// JSON, stays as typed.
fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let only_empty = |paragraph: &str| {
        paragraph
            .trim()
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
            .and_then(&value)
            .is_some_and(|text| text.is_empty())
    };
    let kept = template
        .split("\n\n")
        .filter(|paragraph| !only_empty(paragraph))
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut text = String::with_capacity(kept.len());
    let mut rest = kept.as_str();
    while let Some(open) = rest.find('{') {
        text.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let filled = after
            .find('}')
            .and_then(|close| Some((value(&after[..close])?, close)));
        match filled {
            Some((filled, close)) => {
                text.push_str(&filled);
                rest = &after[close + 1..];
            }
            None => {
                text.push('{');
                rest = after;
            }
        }
    }
    text.push_str(rest);
    text
}

/// The `{words}` of `template` that look like variables and are not one of
/// [`VARIABLES`], each once, for Settings to point out.
pub fn unknown_variables(template: &str) -> Vec<String> {
    let mut unknown: Vec<String> = Vec::new();
    for piece in template.split('{').skip(1) {
        let Some((name, _)) = piece.split_once('}') else {
            continue;
        };
        let looks_like_one =
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        let known = VARIABLES.iter().any(|(variable, _)| *variable == name);
        if looks_like_one && !known && !unknown.iter().any(|seen| seen == name) {
            unknown.push(name.to_owned());
        }
    }
    unknown
}

/// The first reasons, and how many more there are.
fn summary(risk: &[String]) -> String {
    const SHOWN: usize = 5;
    let shown = risk
        .iter()
        .take(SHOWN)
        .map(|reason| format!("`{reason}`"))
        .collect::<Vec<_>>()
        .join(", ");
    match risk.len().saturating_sub(SHOWN) {
        0 => shown,
        more => format!("{shown} e mais {more}"),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use loupe_github::{PrRef, RepoRef};
    use loupe_t3::{Project, ReviewFacts, Session};

    use super::{
        DEFAULT_PROMPT, OPERATE_HINT, Readiness, ReviewSettings, Reviewer, RiskList, assess,
        branch, format_list, precheck, request, risk, thread_title, unknown_variables,
    };
    use crate::links::PrLinkContext;

    fn context() -> PrLinkContext {
        PrLinkContext {
            pr: PrRef::new(RepoRef::new("hooli-inc", "hooli-api"), 374),
            url: "https://github.com/hooli-inc/hooli-api/pull/374".into(),
            title: "feat: trusted CA roots".into(),
            body: String::new(),
            head_ref: "feature/eng-261-p1".into(),
            head_oid: "4914142cdeadbeef".into(),
            head_fork: None,
            author: Some("octocat".into()),
            viewer_login: "IsraelAraujo70".into(),
            labels: Vec::new(),
        }
    }

    fn project(id: &str, repository: &str) -> Project {
        Project {
            id: id.into(),
            title: id.into(),
            repository: Some(repository.into()),
        }
    }

    fn models(pairs: &[(&str, &str)]) -> ReviewFacts {
        ReviewFacts {
            instances: pairs
                .iter()
                .map(|(model, instance)| ((*model).to_owned(), (*instance).to_owned()))
                .collect(),
            branches: HashMap::new(),
        }
    }

    const OPERATE: Session = Session { can_operate: true };

    #[test]
    fn the_button_is_only_for_someone_elses_pull_request() {
        let mut own = context();
        assert!(!own.is_own());
        own.author = Some("israelaraujo70".into());
        assert!(own.is_own(), "logins ignore case");
        own.author = None;
        assert!(!own.is_own());
    }

    #[test]
    fn a_read_only_token_disables_the_button_with_the_settings_sentence() {
        let readiness = assess(
            &context(),
            Session { can_operate: false },
            &[project("p", "github.com/hooli-inc/hooli-api")],
            &models(&[("claude-opus-5-5", "pool")]),
            &[],
            &ReviewSettings::default(),
        );
        assert_eq!(readiness, Readiness::Unavailable(OPERATE_HINT.to_owned()));
    }

    #[test]
    fn a_repository_without_a_project_explains_and_starts_nothing() {
        let readiness = assess(
            &context(),
            OPERATE,
            &[project("p", "github.com/hooli-inc/hooli-web")],
            &models(&[("claude-opus-5-5", "pool")]),
            &[],
            &ReviewSettings::default(),
        );
        match readiness {
            Readiness::Unavailable(reason) => {
                assert!(reason.contains("hooli-inc/hooli-api"), "{reason}");
            }
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    #[test]
    fn the_project_is_the_one_that_tracks_the_repository() {
        let projects = [
            project("web", "github.com/hooli-inc/hooli-web"),
            project("api", "github.com/hooli-inc/hooli-api"),
        ];
        let Readiness::Ready(plan) = assess(
            &context(),
            OPERATE,
            &projects,
            &models(&[("claude-opus-5-5", "pool")]),
            &[],
            &ReviewSettings::default(),
        ) else {
            panic!("expected Ready");
        };
        assert_eq!(plan.project_id, "api");
        // Fable runs on the instance Opus ran on until a Fable thread exists.
        assert_eq!(plan.model(Reviewer::GeneralAndSecurity).instance_id, "pool");
        assert_eq!(
            plan.model(Reviewer::General).model,
            "claude-opus-5-5".to_owned()
        );
        let fable = plan.model(Reviewer::GeneralAndSecurity);
        assert_eq!(
            (fable.model.as_str(), fable.effort.as_deref()),
            ("claude-fable-5-2", Some("medium"))
        );
    }

    #[test]
    fn the_branch_is_one_no_thread_of_the_project_holds() {
        let mut facts = models(&[("claude-opus-5-5", "pool")]);
        facts.branches.insert(
            "api".into(),
            vec!["review/pr-374".into(), "feat/other".into()],
        );
        facts
            .branches
            .insert("web".into(), vec!["review/pr-374-2".into()]);
        let projects = [
            project("web", "github.com/hooli-inc/hooli-web"),
            project("api", "github.com/hooli-inc/hooli-api"),
        ];
        let Readiness::Ready(plan) = assess(
            &context(),
            OPERATE,
            &projects,
            &facts,
            &[],
            &ReviewSettings::default(),
        ) else {
            panic!("expected Ready");
        };
        assert_eq!(plan.branch, "review/pr-374-2");
    }

    #[test]
    fn no_claude_thread_yet_means_no_known_provider() {
        let readiness = assess(
            &context(),
            OPERATE,
            &[project("p", "github.com/hooli-inc/hooli-api")],
            &models(&[]),
            &[],
            &ReviewSettings::default(),
        );
        assert!(matches!(readiness, Readiness::Unavailable(_)));
    }

    #[test]
    fn a_fork_head_is_refused_before_asking_the_server() {
        assert_eq!(precheck(&context()), None);
        let mut fork = context();
        fork.head_fork = Some("octocat/hooli-api".into());
        assert!(matches!(precheck(&fork), Some(Readiness::Unavailable(_))));
    }

    #[test]
    fn a_risk_label_or_a_sensitive_file_makes_the_pull_request_risky() {
        let defaults = ReviewSettings::default();
        let labels = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            risk(&defaults, &labels(&["revisao:risco", "enhancement"]), &[]),
            ["label revisao:risco"]
        );
        assert_eq!(
            risk(&defaults, &labels(&["Security"]), &[]),
            ["label Security"]
        );
        for path in [
            "src/auth/guard.ts",
            "src/modules/PaymentService.ts",
            "src/users/permissions.ts",
            "src/storage/upload-handler.ts",
            "config/secrets.yml",
            "src/billing/trusted-ca-roots.cert.ts",
            "src/pix/cobranca.ts",
            "src/customers/personal-data.ts",
        ] {
            assert_eq!(risk(&defaults, &[], &[path.to_owned()]), [path], "{path}");
        }
        for path in [
            "README.md",
            "src/ui/PixelGrid.tsx",
            "src/search/tokenizer.ts",
            "src/posts/author.ts",
            "src/reports/monthly.ts",
            "src/ui/Button.tsx",
        ] {
            assert!(
                risk(&defaults, &[], &[path.to_owned()]).is_empty(),
                "{path}"
            );
        }
    }

    #[test]
    fn a_risky_pull_request_starts_on_the_security_review() {
        let ready_with = |files: &[&str], settings: &ReviewSettings| {
            let files: Vec<String> = files.iter().map(|path| (*path).to_owned()).collect();
            match assess(
                &context(),
                OPERATE,
                &[project("p", "github.com/hooli-inc/hooli-api")],
                &models(&[("claude-opus-5-5", "pool")]),
                &files,
                settings,
            ) {
                Readiness::Ready(plan) => plan,
                other => panic!("expected Ready, got {other:?}"),
            }
        };
        let ready = |files: &[&str]| ready_with(files, &ReviewSettings::default());
        assert_eq!(ready(&["README.md"]).default_reviewer(), Reviewer::General);
        assert_eq!(
            ready(&["src/auth/session.ts"]).default_reviewer(),
            Reviewer::GeneralAndSecurity
        );
        let off = ReviewSettings {
            risk_check: false,
            ..ReviewSettings::default()
        };
        let plan = ready_with(&["src/auth/session.ts"], &off);
        assert!(plan.risk.is_empty(), "{:?}", plan.risk);
        assert_eq!(plan.default_reviewer(), Reviewer::General);
    }

    #[test]
    fn the_risk_rules_are_the_users_lists() {
        let mut settings = ReviewSettings::default();
        settings.set_list(RiskList::Labels, "needs care, Hotfix");
        settings.set_list(RiskList::Words, "migration,\nschema");
        settings.set_list(RiskList::Exceptions, "schemat");
        let labels = vec!["risk".to_owned(), "NEEDS CARE: db".to_owned()];
        let files: Vec<String> = [
            "db/Migration_001.sql",
            "src/auth/login.ts",
            "docs/schematics.md",
            "src/SchemaCache.rs",
        ]
        .map(str::to_owned)
        .to_vec();
        assert_eq!(
            risk(&settings, &labels, &files),
            [
                "label NEEDS CARE: db",
                "db/Migration_001.sql",
                "src/SchemaCache.rs"
            ],
            "the defaults no longer apply, and a list matches in any case"
        );
        settings.set_list(RiskList::Words, "");
        settings.set_list(RiskList::Labels, "");
        assert!(
            risk(&settings, &labels, &files).is_empty(),
            "an empty list marks nothing"
        );
    }

    #[test]
    fn a_setting_equal_to_the_default_stays_the_default() {
        let mut settings = ReviewSettings::default();
        settings.set_prompt(DEFAULT_PROMPT);
        assert_eq!(settings.prompt, None);
        settings.set_prompt("Review #{number}");
        assert_eq!(settings.prompt(), "Review #{number}");
        for list in [RiskList::Labels, RiskList::Words, RiskList::Exceptions] {
            let shown = format_list(&settings.list(list));
            settings.set_list(list, &shown);
            assert_eq!(settings.list(list), ReviewSettings::default().list(list));
        }
        assert_eq!(
            (
                &settings.risk_labels,
                &settings.risk_words,
                &settings.risk_exceptions
            ),
            (&None, &None, &None)
        );
        settings.set_list(RiskList::Words, "auth,\n auth , ,Auth, pix");
        assert_eq!(settings.list(RiskList::Words), ["auth", "pix"]);
    }

    #[test]
    fn an_older_settings_file_gets_the_default_review_settings() {
        let review: ReviewSettings = serde_json::from_str("{}").expect("parse");
        assert_eq!(review, ReviewSettings::default());
        assert!(review.risk_check, "the risk check starts on");
        let settings: crate::store::Settings =
            serde_json::from_str(r#"{"t3_server_url":"https://t3.example"}"#).expect("parse");
        assert_eq!(settings.t3_review, ReviewSettings::default());
    }

    #[test]
    fn a_template_fills_each_variable_once_and_keeps_other_braces() {
        let template = "#{number} {title} by {author}\n{url} {repository} {head_branch} \
                        {head_sha} {short_sha}\n{\"json\":1} {unknown} {}";
        assert_eq!(
            request(&context(), Reviewer::General, &[], template),
            "#374 feat: trusted CA roots by octocat\n\
             https://github.com/hooli-inc/hooli-api/pull/374 hooli-inc/hooli-api \
             feature/eng-261-p1 4914142cdeadbeef 4914142c\n{\"json\":1} {unknown} {}"
        );
        assert_eq!(unknown_variables(template), ["unknown"]);
        assert!(unknown_variables(DEFAULT_PROMPT).is_empty());
        assert_eq!(unknown_variables("{titel} {titel} {number}"), ["titel"]);
    }

    #[test]
    fn the_security_paragraph_goes_where_the_template_puts_it() {
        let template = "Review it.\n\n{security}\n\nThen stop.";
        assert_eq!(
            request(&context(), Reviewer::General, &[], template),
            "Review it.\n\nThen stop.",
            "an empty paragraph leaves with its blank line"
        );
        let text = request(&context(), Reviewer::GeneralAndSecurity, &[], template);
        assert!(text.starts_with("Review it.\n\nO usuário pediu"), "{text}");
        assert!(text.ends_with("essa skill.\n\nThen stop."), "{text}");
        let without = request(
            &context(),
            Reviewer::GeneralAndSecurity,
            &[],
            "Review #{number}.",
        );
        assert_eq!(without, "Review #374.", "a template without it has none");
    }

    #[test]
    fn the_request_asks_for_the_seven_points_of_the_review() {
        let text = request(&context(), Reviewer::General, &[], DEFAULT_PROMPT);
        for part in [
            "PR #374 de hooli-inc/hooli-api, de `octocat`",
            "https://github.com/hooli-inc/hooli-api/pull/374",
            "head atual de `origin/feature/eng-261-p1` (era `4914142c`",
            "AGENTS.md e o CLAUDE.md",
            "gh api repos/hooli-inc/hooli-api/pulls/374/reviews",
            "`.claude/agents/`",
            "skills `verify*`",
            "**Resolve o que diz resolver?**",
            "ticket ligado no Linear",
            "arquivo:linha",
            "🔴 bloqueia / 🟡 recomendação",
            "uma mutação relevante faz um teste falhar",
            "os pesados onde o AGENTS.md do repo mandar",
            "**Escopo**",
            "Não comente no PR, não aprove, não faça push nem merge",
            "aprovar, aprovar com ressalvas ou pedir mudanças",
            "{\"veredito\":\"...\",\"vermelhos\":<n>,\"amarelos\":<n>,\"nao_verificado\":\"...\"}",
        ] {
            assert!(text.contains(part), "missing {part:?} in:\n{text}");
        }
        assert!(!text.contains("revisar-seguranca"));
    }

    #[test]
    fn the_security_review_follows_the_skill_and_says_why() {
        let reasons = vec!["label revisao:risco".to_owned(), "src/auth.ts".to_owned()];
        let text = request(
            &context(),
            Reviewer::GeneralAndSecurity,
            &reasons,
            DEFAULT_PROMPT,
        );
        assert!(text.contains("skill `revisar-seguranca`"), "{text}");
        assert!(
            text.contains("Este PR é de risco (`label revisao:risco`, `src/auth.ts`)"),
            "{text}"
        );
        let asked = request(
            &context(),
            Reviewer::GeneralAndSecurity,
            &[],
            DEFAULT_PROMPT,
        );
        assert!(asked.contains("O usuário pediu também a revisão de segurança"));
    }

    #[test]
    fn the_thread_and_its_branch_are_named_after_the_pull_request() {
        assert_eq!(branch(&context(), &[]), "review/pr-374");
        let taken = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            branch(&context(), &taken(&["review/pr-374", "feat/x"])),
            "review/pr-374-2"
        );
        assert_eq!(
            branch(&context(), &taken(&["review/pr-374-2", "review/pr-374"])),
            "review/pr-374-3"
        );
        assert_eq!(
            thread_title(&context()),
            "Revisar PR #374 de octocat: feat: trusted CA roots"
        );
    }
}

/// The requests Loupe sent before the prompt and the risk rules became
/// settings, captured from that build. With the default settings the request
/// must stay byte for byte the same.
#[cfg(test)]
mod golden {
    use loupe_github::{PrRef, RepoRef};

    use super::{ReviewSettings, Reviewer, request};
    use crate::links::PrLinkContext;

    fn context() -> PrLinkContext {
        PrLinkContext {
            pr: PrRef::new(RepoRef::new("hooli-inc", "hooli-api"), 374),
            url: "https://github.com/hooli-inc/hooli-api/pull/374".into(),
            title: "feat: trusted CA roots".into(),
            body: String::new(),
            head_ref: "feature/eng-261-p1".into(),
            head_oid: "4914142cdeadbeef".into(),
            head_fork: None,
            author: Some("octocat".into()),
            viewer_login: "IsraelAraujo70".into(),
            labels: Vec::new(),
        }
    }

    /// The request the dialog writes with the default settings.
    fn default_request(context: &PrLinkContext, reviewer: Reviewer, risk: &[String]) -> String {
        request(context, reviewer, risk, ReviewSettings::default().prompt())
    }

    #[track_caller]
    fn same(actual: String, expected: &str) {
        if actual != expected {
            let at = actual
                .bytes()
                .zip(expected.bytes())
                .position(|(a, b)| a != b)
                .unwrap_or(actual.len().min(expected.len()));
            panic!(
                "the default request changed at byte {at}\n--- expected\n{expected}\n--- actual\n{actual}"
            );
        }
    }

    #[test]
    fn the_default_request_is_byte_for_byte_the_one_loupe_sent_before() {
        let two = vec!["label revisao:risco".to_owned(), "src/auth.ts".to_owned()];
        let seven: Vec<String> = (1..=7).map(|k| format!("src/auth/file{k}.ts")).collect();
        let general = include_str!("golden/general.txt");
        same(default_request(&context(), Reviewer::General, &[]), general);
        same(
            default_request(&context(), Reviewer::General, &two),
            general,
        );
        same(
            default_request(&context(), Reviewer::GeneralAndSecurity, &[]),
            include_str!("golden/security-asked.txt"),
        );
        same(
            default_request(&context(), Reviewer::GeneralAndSecurity, &two),
            include_str!("golden/security-risky.txt"),
        );
        same(
            default_request(&context(), Reviewer::GeneralAndSecurity, &seven),
            include_str!("golden/security-many.txt"),
        );
        let mut odd = context();
        odd.title = "fix: keep {url} and {number} and \"quotes\" as typed".into();
        odd.author = None;
        same(
            default_request(&odd, Reviewer::General, &[]),
            include_str!("golden/general-odd.txt"),
        );
        // Every value that can hold braces holds a variable's name, so a
        // value filled in early can never be filled again by a later one.
        let braces = PrLinkContext {
            url: "https://example.test/{repository}/pull/{number}".into(),
            title: "{head_branch} and {security}".into(),
            head_ref: "fix/{number}-{title}".into(),
            head_oid: "{url}".into(),
            author: Some("{short_sha}".into()),
            ..context()
        };
        let risk = vec!["label {url}".to_owned(), "src/{title}/auth.ts".to_owned()];
        same(
            default_request(&braces, Reviewer::GeneralAndSecurity, &risk),
            include_str!("golden/security-braces.txt"),
        );
        same(
            default_request(&braces, Reviewer::General, &risk),
            include_str!("golden/general-braces.txt"),
        );
    }
}
