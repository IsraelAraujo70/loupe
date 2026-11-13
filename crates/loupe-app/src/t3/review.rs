//! "Review with T3 Code": whether a pull request can get a review thread,
//! which reviewer it gets, and the request the thread starts with. Pure, so
//! every rule here is tested without a server.

use std::collections::HashMap;

use loupe_t3::{ModelChoice, Project, PullRequestKey, ReviewFacts, Session};

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
        risk: risk(&context.labels, files),
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

/// Why a pull request is risky: each risk label, then each changed file that
/// touches a sensitive area. Empty when nothing does.
pub(super) fn risk(labels: &[String], files: &[String]) -> Vec<String> {
    let labels = labels.iter().filter(|label| {
        let label = label.to_lowercase();
        RISK_LABELS.iter().any(|word| label.contains(word))
    });
    let files = files.iter().filter(|path| {
        words(path).iter().any(|word| {
            !NOT_RISK.iter().any(|not| word.starts_with(not))
                && RISK_WORDS.iter().any(|risk| word.starts_with(risk))
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

/// The review request the thread starts with. The user can edit it before
/// sending.
pub(super) fn request(context: &PrLinkContext, reviewer: Reviewer, risk: &[String]) -> String {
    let repo = context.pr.repo.to_string();
    let n = context.pr.number;
    let author = context.author.as_deref().unwrap_or("ghost");
    let head: String = context.head_oid.chars().take(8).collect();
    let mut text = format!(
        "Revisar o PR #{n} de {repo}, de `{author}`: \"{title}\".\n{url}\n\n\
         Esta worktree parte do head atual de `origin/{head_ref}` (era `{head}` quando o \
         pedido foi escrito).\n\n\
         1. Leia o AGENTS.md e o CLAUDE.md do repo, a descrição do PR \
         (`gh pr view {n} -R {repo}`), os comentários e as revisões já feitas \
         (`gh api repos/{repo}/pulls/{n}/comments`, `gh api repos/{repo}/issues/{n}/comments`, \
         `gh api repos/{repo}/pulls/{n}/reviews`).\n\
         2. Use os revisores versionados do repo (`.claude/agents/`) e as skills `verify*` do \
         repo como checklist, quando existirem.\n\
         3. **Resolve o que diz resolver?** Confira contra o ticket ligado no Linear, quando \
         houver.\n\
         4. **Achados**, cada um com arquivo:linha, gravidade (🔴 bloqueia / 🟡 recomendação) \
         e uma prova: um teste, uma reprodução ou o caminho no código.\n\
         5. **Testes**: rode os specs que o PR toca (os pesados onde o AGENTS.md do repo \
         mandar) e mostre que uma mutação relevante faz um teste falhar.\n\
         6. **Escopo**: diga o que vazou para fora do que o PR pede.\n",
        title = context.title,
        url = context.url,
        head_ref = context.head_ref,
    );
    if reviewer == Reviewer::GeneralAndSecurity {
        let why = if risk.is_empty() {
            "O usuário pediu também a revisão de segurança".to_owned()
        } else {
            format!("Este PR é de risco ({})", summary(risk))
        };
        text.push_str(&format!(
            "\n{why}: além da revisão geral, faça a revisão de segurança seguindo a skill \
             `revisar-seguranca`, quando o ambiente tiver essa skill.\n"
        ));
    }
    text.push_str(
        "\nNão comente no PR, não aprove, não faça push nem merge. Termine com um texto pronto \
         para o usuário postar como revisão, o veredito (aprovar, aprovar com ressalvas ou pedir \
         mudanças) e um bloco json: \
         {\"veredito\":\"...\",\"vermelhos\":<n>,\"amarelos\":<n>,\"nao_verificado\":\"...\"}.",
    );
    text
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
        OPERATE_HINT, Readiness, Reviewer, assess, branch, precheck, request, risk, thread_title,
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
        let Readiness::Ready(plan) = assess(&context(), OPERATE, &projects, &facts, &[]) else {
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
        let labels = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            risk(&labels(&["revisao:risco", "enhancement"]), &[]),
            ["label revisao:risco"]
        );
        assert_eq!(risk(&labels(&["Security"]), &[]), ["label Security"]);
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
            assert_eq!(risk(&[], &[path.to_owned()]), [path], "{path}");
        }
        for path in [
            "README.md",
            "src/ui/PixelGrid.tsx",
            "src/search/tokenizer.ts",
            "src/posts/author.ts",
            "src/reports/monthly.ts",
            "src/ui/Button.tsx",
        ] {
            assert!(risk(&[], &[path.to_owned()]).is_empty(), "{path}");
        }
    }

    #[test]
    fn a_risky_pull_request_starts_on_the_security_review() {
        let ready = |files: &[&str]| {
            let files: Vec<String> = files.iter().map(|path| (*path).to_owned()).collect();
            match assess(
                &context(),
                OPERATE,
                &[project("p", "github.com/hooli-inc/hooli-api")],
                &models(&[("claude-opus-5-5", "pool")]),
                &files,
            ) {
                Readiness::Ready(plan) => plan,
                other => panic!("expected Ready, got {other:?}"),
            }
        };
        assert_eq!(ready(&["README.md"]).default_reviewer(), Reviewer::General);
        assert_eq!(
            ready(&["src/auth/session.ts"]).default_reviewer(),
            Reviewer::GeneralAndSecurity
        );
    }

    #[test]
    fn the_request_asks_for_the_seven_points_of_the_review() {
        let text = request(&context(), Reviewer::General, &[]);
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
        let text = request(&context(), Reviewer::GeneralAndSecurity, &reasons);
        assert!(text.contains("skill `revisar-seguranca`"), "{text}");
        assert!(
            text.contains("Este PR é de risco (`label revisao:risco`, `src/auth.ts`)"),
            "{text}"
        );
        let asked = request(&context(), Reviewer::GeneralAndSecurity, &[]);
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

    use super::{Reviewer, request};
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
        request(context, reviewer, risk)
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
    }
}
