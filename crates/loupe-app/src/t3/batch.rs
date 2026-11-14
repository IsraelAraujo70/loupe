//! "Review with T3 Code" for the pull requests picked on Reviews: one dialog
//! with the review prompt tabs and the shared request, the pull requests it
//! starts, each with its readiness, and one thread per ready pull request.
//! The list then reports what started and what failed.

use std::cell::Cell;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{GitHub, PrRef, PrSummary};
use loupe_t3::{LaunchedReview, ModelChoice, Provider, PullRequestKey, ReviewLaunch, T3};

use super::dialog::{failure_text, prompt_tabs, risk_line, runs_on};
use super::review::{self, Candidate, Readiness, ReviewPrompt, ReviewSettings, Server};
use crate::links::PrLinkContext;
use crate::services::{self, Services};
use crate::store::Settings;
use crate::theme::scale::{self, dp};

/// Starts one review thread; T3's `launch_review` outside tests.
type Launcher = Arc<dyn Fn(&ReviewLaunch) -> loupe_t3::Result<LaunchedReview> + Send + Sync>;

/// How many pull requests are read from GitHub at once.
const GITHUB_AT_ONCE: usize = 6;

/// Opens the dialog for `picked`, in the order on screen, and reads each
/// pull request and the T3 server behind it.
pub(super) fn open(picked: Vec<PrSummary>, window: &mut Window, cx: &mut App) {
    let services = Services::get(cx);
    let (Some(t3), Some(github)) = (services.t3.clone(), services.github.clone()) else {
        window.push_notification(Notification::warning(review::CONNECT_HINT), cx);
        return;
    };
    let launcher: Launcher = {
        let t3 = t3.clone();
        Arc::new(move |launch| t3.launch_review(launch))
    };
    let picked = picked
        .into_iter()
        .map(|summary| (summary.pr, summary.title))
        .collect();
    let form = cx.new(|cx| {
        let mut form = BatchForm::new(picked, launcher, window, cx);
        form.load(t3, github, window, cx);
        form
    });
    let text_focus = form.read(cx).text.clone();
    let focus_text = Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus_text.replace(false) {
            text_focus.focus_handle(cx).focus(window, cx);
        }
        dialog
            .title("Review with T3 Code")
            .w(dp(640.))
            .overlay_closable(false)
            .keyboard(false)
            .close_button(false)
            .child(form.clone())
    });
}

/// Where one pull request's thread stands.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Launch {
    Idle,
    Sending,
    /// Running; `Some` says why T3 did not link it to the pull request.
    Started(Option<String>),
    Failed(String),
}

struct Item {
    pr: PrRef,
    /// The title Reviews showed, until GitHub's detail arrives.
    title: String,
    context: Option<PrLinkContext>,
    readiness: Readiness,
    launch: Launch,
    /// Kept for the life of the dialog: a retry after a lost answer does
    /// not start a second thread.
    request_id: String,
}

impl Item {
    /// Ready and not started yet, or failed: what Start sends.
    fn pending(&self) -> Option<(&PrLinkContext, &review::Plan)> {
        match (&self.readiness, &self.context, &self.launch) {
            (Readiness::Ready(plan), Some(context), Launch::Idle | Launch::Failed(_)) => {
                Some((context, plan))
            }
            _ => None,
        }
    }
}

struct BatchForm {
    items: Vec<Item>,
    launcher: Launcher,
    /// The server's providers; `None` until the server answered.
    providers: Option<Vec<Provider>>,
    /// The review prompts of Settings > T3 Code when the dialog opened.
    prompts: Vec<ReviewPrompt>,
    selected: usize,
    /// The template of Settings > T3 Code when the dialog opened.
    template: String,
    /// Whether the user picked a tab, which the risk prompt then leaves.
    picked_tab: bool,
    text: Entity<TextareaState>,
    error: Option<SharedString>,
    _load: Option<Task<()>>,
}

/// What the dialog learned about every pull request at once.
struct Loaded {
    contexts: Vec<Option<PrLinkContext>>,
    readiness: Vec<Readiness>,
    providers: Option<Vec<Provider>>,
}

impl BatchForm {
    /// The dialog for `picked` (each pull request with the title Reviews
    /// showed), every one still being checked.
    fn new(
        picked: Vec<(PrRef, String)>,
        launcher: Launcher,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let review = &Settings::get(cx).t3_review;
        let template = review.prompt().to_owned();
        let prompts = review.prompts();
        let selected = review.starting_index(false);
        let initial = review::shared_request(&template, &prompts[selected].text);
        let text = cx.new(|cx| {
            let mut state = TextareaState::new(window, cx).auto_grow(8, 14);
            state.set_value(initial, window, cx);
            state
        });
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let items = picked
            .into_iter()
            .map(|(pr, title)| Item {
                request_id: format!("loupe-review:{}:{}:{nonce}", pr.repo, pr.number),
                pr,
                title,
                context: None,
                readiness: Readiness::Checking,
                launch: Launch::Idle,
            })
            .collect();
        BatchForm {
            items,
            launcher,
            providers: None,
            prompts,
            selected,
            template,
            picked_tab: false,
            text,
            error: None,
            _load: None,
        }
    }

    /// Reads each pull request from GitHub and the T3 server once.
    fn load(
        &mut self,
        t3: Arc<T3>,
        github: Arc<GitHub>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let prs: Vec<PrRef> = self.items.iter().map(|item| item.pr.clone()).collect();
        let settings = Settings::get(cx).t3_review.clone();
        let work = services::background(cx, move || load(&t3, &github, &prs, &settings));
        self._load = Some(cx.spawn_in(window, async move |this, cx| {
            let loaded = work.await;
            this.update_in(cx, |form, window, cx| form.apply(loaded, window, cx))
                .ok();
        }));
    }

    /// Shows what the load found. A risky ready pull request moves the
    /// dialog to the risk prompt, unless the user picked a tab.
    fn apply(&mut self, loaded: Loaded, window: &mut Window, cx: &mut Context<Self>) {
        for ((item, context), readiness) in self
            .items
            .iter_mut()
            .zip(loaded.contexts)
            .zip(loaded.readiness)
        {
            item.context = context;
            item.readiness = readiness;
        }
        self.providers = loaded.providers;
        let risky = self
            .items
            .iter()
            .any(|item| matches!(&item.readiness, Readiness::Ready(plan) if !plan.risk.is_empty()));
        if risky && !self.picked_tab {
            let index = Settings::get(cx).t3_review.starting_index(true);
            self.select(index, window, cx);
        }
        cx.notify();
    }

    fn sending(&self) -> bool {
        self.items.iter().any(|item| item.launch == Launch::Sending)
    }

    /// The model every thread runs on, or why the server cannot run it.
    fn model(&self) -> Option<Result<ModelChoice, String>> {
        let providers = self.providers.as_ref()?;
        Some(review::model_choice(
            providers,
            &self.prompts[self.selected],
        ))
    }

    fn shared_for(&self, index: usize) -> String {
        review::shared_request(&self.template, &self.prompts[index].text)
    }

    fn pick(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.sending() {
            return;
        }
        self.picked_tab = true;
        self.select(index, window, cx);
    }

    /// Opens another prompt's tab. The request follows unless the user
    /// edited it.
    fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if index == self.selected || index >= self.prompts.len() {
            return;
        }
        if self.text.read(cx).value() == self.shared_for(self.selected) {
            let next = self.shared_for(index);
            self.text
                .update(cx, |state, cx| state.set_value(next, window, cx));
        }
        self.selected = index;
        self.error = None;
        cx.notify();
    }

    /// Starts a thread for every pending pull request, each on its own;
    /// one that fails leaves the others running.
    fn send(&mut self, cx: &mut Context<Self>) {
        if self.sending() {
            return;
        }
        let model = match self.model() {
            Some(Ok(model)) => model,
            Some(Err(why)) => {
                self.error = Some(why.into());
                cx.notify();
                return;
            }
            None => return,
        };
        let shared = self.text.read(cx).value().trim().to_owned();
        if shared.is_empty() {
            self.error = Some("Write the review request first.".into());
            cx.notify();
            return;
        }
        self.error = None;
        let launches: Vec<(usize, ReviewLaunch)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let (context, plan) = item.pending()?;
                Some((
                    index,
                    ReviewLaunch {
                        request_id: item.request_id.clone(),
                        project_id: plan.project_id.clone(),
                        title: review::thread_title(context),
                        message: review::fill_request(context, &plan.risk, &shared),
                        model: model.clone(),
                        base_ref: context.head_ref.clone(),
                        branch: plan.branch.clone(),
                        pull_request: PullRequestKey::github(
                            context.pr.repo.to_string(),
                            context.pr.number,
                        ),
                        pull_request_url: context.url.clone(),
                    },
                ))
            })
            .collect();
        for (index, launch) in launches {
            self.items[index].launch = Launch::Sending;
            let launcher = self.launcher.clone();
            let task = services::background(cx, move || launcher(&launch));
            cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |form, cx| form.finish(index, result, cx))
                    .ok();
            })
            .detach();
        }
        cx.notify();
    }

    fn finish(
        &mut self,
        index: usize,
        result: loupe_t3::Result<LaunchedReview>,
        cx: &mut Context<Self>,
    ) {
        self.items[index].launch = match result {
            Ok(launched) => Launch::Started(launched.link_error.map(|error| error.to_string())),
            Err(error) => Launch::Failed(failure_text(&error)),
        };
        cx.notify();
    }

    fn counts(&self) -> Counts {
        let mut counts = Counts::default();
        for item in &self.items {
            match (&item.readiness, &item.launch) {
                (Readiness::Checking, _) => counts.checking += 1,
                (Readiness::Unavailable(_), _) => counts.skipped += 1,
                (Readiness::Ready(_), Launch::Idle) => counts.ready += 1,
                (Readiness::Ready(_), Launch::Sending) => counts.sending += 1,
                (Readiness::Ready(_), Launch::Started(_)) => counts.started += 1,
                (Readiness::Ready(_), Launch::Failed(_)) => counts.failed += 1,
            }
        }
        counts
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Counts {
    checking: usize,
    skipped: usize,
    ready: usize,
    sending: usize,
    started: usize,
    failed: usize,
}

impl Counts {
    /// The line over the list: what will start, what started, what not.
    fn summary(&self) -> String {
        let parts = [
            (self.checking, "checking"),
            (self.ready, "ready"),
            (self.sending, "starting"),
            (self.started, "started"),
            (self.failed, "failed"),
            (self.skipped, "skipped"),
        ];
        parts
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, word)| format!("{count} {word}"))
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

/// Reads every pull request (a few at a time) and the server, then assesses
/// them all in the order on screen.
fn load(t3: &T3, github: &GitHub, prs: &[PrRef], settings: &ReviewSettings) -> Loaded {
    let (server, candidates) = std::thread::scope(|scope| {
        let server = scope.spawn(|| Server::load(t3));
        let candidates: Vec<Result<Candidate, String>> = prs
            .chunks(GITHUB_AT_ONCE)
            .flat_map(|chunk| {
                let reads: Vec<_> = chunk
                    .iter()
                    .map(|pr| scope.spawn(move || candidate(github, pr)))
                    .collect();
                reads
                    .into_iter()
                    .map(|read| {
                        read.join()
                            .unwrap_or_else(|_| Err("the read stopped unexpectedly".to_owned()))
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        let server = server
            .join()
            .unwrap_or_else(|_| Err("Could not check T3.".to_owned()));
        (server, candidates)
    });
    Loaded {
        readiness: review::assess_batch(&candidates, &server, settings),
        contexts: candidates
            .into_iter()
            .map(|candidate| candidate.ok().map(|candidate| candidate.context))
            .collect(),
        providers: server.ok().map(|server| server.providers),
    }
}

/// One pull request as the review needs it. The changed files are read
/// only for one that can be reviewed.
fn candidate(github: &GitHub, pr: &PrRef) -> Result<Candidate, String> {
    let detail = github.pull_request(pr).map_err(|error| error.to_string())?;
    let context = PrLinkContext::from_detail(&detail);
    let files = if review::not_reviewable(&context).is_none() && context.head_fork.is_none() {
        github
            .pull_request_files(pr)
            .map(|files| files.into_iter().map(|file| file.path).collect())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(Candidate { context, files })
}

impl Render for BatchForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sending = self.sending();
        let counts = self.counts();
        let model = self.model();
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let tabs = prompt_tabs(
            &self.prompts,
            self.selected,
            self.providers.as_deref(),
            sending,
            Self::pick,
            cx,
        );
        let runs_on = match (&model, &self.providers) {
            (Some(model), Some(providers)) => runs_on(model, providers, cx),
            _ => div()
                .text_size(scale::meta())
                .text_color(muted)
                .child("Checking T3\u{2026}")
                .into_any_element(),
        };
        let rows: Vec<AnyElement> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| row(index, item, cx))
            .collect();
        let pending = self
            .items
            .iter()
            .filter(|item| item.pending().is_some())
            .count();
        let started_any = counts.started + counts.failed > 0;
        let start_label = match (sending, counts.failed > 0 && counts.ready == 0, pending) {
            (true, _, _) => "Starting\u{2026}".to_owned(),
            (false, true, count) => format!("Retry {count}"),
            (false, false, 1) => "Start 1 review".to_owned(),
            (false, false, count) => format!("Start {count} reviews"),
        };
        let can_start = !sending && pending > 0 && matches!(model, Some(Ok(_)));
        v_flex()
            .gap_3()
            .child(v_flex().gap_2().child(tabs).child(runs_on))
            .child(Textarea::new(&self.text).disabled(sending || (started_any && pending == 0)))
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        div()
                            .text_size(scale::meta())
                            .text_color(muted)
                            .debug_selector(|| "t3-batch-summary".into())
                            .child(counts.summary()),
                    )
                    .child(
                        v_flex()
                            .id("t3-batch-list")
                            .max_h(rems(17.5))
                            .overflow_y_scroll()
                            .children(rows),
                    ),
            )
            .children(
                self.error
                    .clone()
                    .map(|message| div().text_sm().text_color(danger).child(message)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1())
                    .child(
                        Button::new("t3-batch-cancel")
                            .ghost()
                            .small()
                            .label(if started_any { "Close" } else { "Cancel" })
                            .disabled(sending)
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .when(pending > 0 || sending, |footer| {
                        footer.child(
                            Button::new("t3-batch-start")
                                .debug_selector(|| "t3-batch-start".into())
                                .primary()
                                .small()
                                .label(start_label)
                                .loading(sending)
                                .disabled(!can_start)
                                .on_click(cx.listener(|form, _, _, cx| form.send(cx))),
                        )
                    }),
            )
    }
}

/// One pull request of the list: its reference and title, and under them
/// where its thread would run, why it is skipped, or what happened.
fn row(index: usize, item: &Item, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let (muted, danger, success, warning) = (
        theme.muted_foreground,
        theme.danger,
        theme.success,
        theme.warning,
    );
    let title = item
        .context
        .as_ref()
        .map_or(item.title.clone(), |context| context.title.clone());
    let (icon, color, status, tooltip): (IconName, Hsla, String, Option<String>) =
        match (&item.readiness, &item.launch) {
            (Readiness::Checking, _) => (
                IconName::CircleDashed,
                muted,
                "Checking\u{2026}".to_owned(),
                None,
            ),
            (Readiness::Unavailable(why), _) => (
                IconName::CircleSlash,
                muted,
                format!("Skipped: {why}"),
                None,
            ),
            (Readiness::Ready(plan), Launch::Idle) => {
                let place = format!("{} in {}", plan.branch, plan.project_title);
                match risk_line(&plan.risk) {
                    Some(risk) => (IconName::TriangleAlert, warning, place, Some(risk)),
                    None => (IconName::CircleDot, muted, place, None),
                }
            }
            (Readiness::Ready(plan), Launch::Sending) => (
                IconName::CircleDashed,
                muted,
                format!("Starting {}\u{2026}", plan.branch),
                None,
            ),
            (Readiness::Ready(plan), Launch::Started(None)) => (
                IconName::CircleCheck,
                success,
                format!("Started on {}", plan.branch),
                None,
            ),
            (Readiness::Ready(plan), Launch::Started(Some(why))) => (
                IconName::CircleCheck,
                warning,
                format!(
                    "Started on {}, but T3 did not link it to the pull request: {why}",
                    plan.branch
                ),
                None,
            ),
            (Readiness::Ready(_), Launch::Failed(why)) => {
                (IconName::CircleX, danger, format!("Failed: {why}"), None)
            }
        };
    let failed = matches!(item.launch, Launch::Failed(_));
    h_flex()
        .id(("t3-batch-row", index))
        .debug_selector(move || format!("t3-batch-row-{index}"))
        .w_full()
        .py_1()
        .gap_2()
        .items_start()
        .when_some(tooltip, |row, tip| {
            row.tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        })
        .child(
            h_flex()
                .flex_none()
                .h_5()
                .items_center()
                .child(Icon::new(icon).with_size(dp(16.)).text_color(color)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    h_flex()
                        .h_5()
                        .gap_2()
                        .items_center()
                        .text_size(scale::body())
                        .child(
                            div()
                                .flex_none()
                                .text_color(muted)
                                .child(format!("{} #{}", item.pr.repo, item.pr.number)),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(title)),
                )
                .child(
                    div()
                        .text_size(scale::meta())
                        .text_color(if failed { danger } else { muted })
                        .child(status),
                ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use gpui_kit::{
        AppContext as _, Bounds, Entity, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };
    use loupe_github::{PrRef, RepoRef};
    use loupe_t3::{
        Effort, Error, LaunchedReview, Project, Provider, ProviderModel, ReviewFacts, ReviewLaunch,
        Session,
    };

    use super::{BatchForm, Counts, Launch, Launcher, Loaded};
    use crate::links::PrLinkContext;
    use crate::store::Settings;
    use crate::t3::review::{self, Candidate, Readiness, Server};

    fn context(repo: &str, number: u64) -> PrLinkContext {
        PrLinkContext {
            pr: PrRef::new(RepoRef::new("acme", repo), number),
            url: format!("https://github.com/acme/{repo}/pull/{number}"),
            title: format!("Change {number}"),
            body: String::new(),
            head_ref: format!("fix/{number}"),
            head_oid: "abc123".into(),
            head_fork: None,
            author: Some("hubot".into()),
            viewer_login: "octocat".into(),
            labels: Vec::new(),
        }
    }

    fn server() -> Server {
        let efforts = ["low", "medium", "high"]
            .map(|id| Effort {
                id: id.into(),
                label: id.into(),
                is_default: id == "medium",
            })
            .to_vec();
        let model = |slug: &str| ProviderModel {
            slug: slug.into(),
            name: slug.into(),
            efforts: efforts.clone(),
        };
        Server {
            session: Session { can_operate: true },
            projects: vec![Project {
                id: "app".into(),
                title: "app".into(),
                repository: Some("github.com/acme/app".into()),
            }],
            facts: ReviewFacts {
                branches: [("app".to_owned(), vec!["review/pr-1".to_owned()])].into(),
            },
            providers: vec![Provider {
                instance_id: "pool".into(),
                name: "Pool".into(),
                unavailable: None,
                models: vec![model("claude-opus-5-5"), model("claude-fable-5-2")],
            }],
        }
    }

    fn of(context: PrLinkContext, files: &[&str]) -> Result<Candidate, String> {
        Ok(Candidate {
            context,
            files: files.iter().map(|path| (*path).to_owned()).collect(),
        })
    }

    #[test]
    fn a_batch_skips_what_the_single_button_refuses_and_gives_each_a_branch() {
        let mut fork = context("app", 3);
        fork.head_fork = Some("someone/app".into());
        let mut own = context("app", 4);
        own.author = Some("OctoCat".into());
        let mut ghost = context("app", 5);
        ghost.author = None;
        let candidates = vec![
            of(context("app", 1), &[]),
            of(context("app", 2), &["src/auth/session.ts"]),
            of(fork, &[]),
            of(own, &[]),
            of(ghost, &[]),
            of(context("web", 6), &[]),
            Err("rate limited".to_owned()),
        ];
        let readiness = review::assess_batch(
            &candidates,
            &Ok(server()),
            &review::ReviewSettings::default(),
        );
        let plans: Vec<(String, usize)> = readiness[..2]
            .iter()
            .map(|readiness| match readiness {
                Readiness::Ready(plan) => (plan.branch.clone(), plan.risk.len()),
                other => panic!("expected Ready, got {other:?}"),
            })
            .collect();
        assert_eq!(
            plans,
            [
                ("review/pr-1-2".to_owned(), 0),
                ("review/pr-2".to_owned(), 1)
            ],
            "a branch a thread holds is not reused, and each pull request has its own risk"
        );
        let skipped: Vec<String> = readiness[2..]
            .iter()
            .map(|readiness| match readiness {
                Readiness::Unavailable(why) => why.clone(),
                other => panic!("expected Unavailable, got {other:?}"),
            })
            .collect();
        assert!(skipped[0].contains("fork someone/app"), "{}", skipped[0]);
        assert!(skipped[1].contains("You opened it"), "{}", skipped[1]);
        assert!(skipped[2].contains("deleted"), "{}", skipped[2]);
        assert!(
            skipped[3].contains("No T3 project tracks acme/web"),
            "{}",
            skipped[3]
        );
        assert_eq!(skipped[4], "GitHub did not load it: rate limited");

        let down = review::assess_batch(
            &candidates[..3],
            &Err("Could not check T3: offline".to_owned()),
            &review::ReviewSettings::default(),
        );
        assert_eq!(
            down[0],
            Readiness::Unavailable("Could not check T3: offline".into())
        );
        assert!(
            matches!(&down[2], Readiness::Unavailable(why) if why.contains("fork")),
            "a fork says so even when the server is down"
        );
    }

    #[test]
    fn two_pull_requests_of_one_batch_never_share_a_branch() {
        let mut server = server();
        server.facts.branches.clear();
        // The same number in one project can only come from a second look
        // at the same pull request; it still gets a branch of its own.
        let candidates = vec![of(context("app", 9), &[]), of(context("app", 9), &[])];
        let branches: Vec<String> =
            review::assess_batch(&candidates, &Ok(server), &review::ReviewSettings::default())
                .into_iter()
                .map(|readiness| match readiness {
                    Readiness::Ready(plan) => plan.branch,
                    other => panic!("expected Ready, got {other:?}"),
                })
                .collect();
        assert_eq!(branches, ["review/pr-9", "review/pr-9-2"]);
    }

    #[test]
    fn the_shared_request_fills_like_the_single_one() {
        let settings = review::ReviewSettings::default();
        let mut risky = context("app", 2);
        risky.title = "{url} stays as typed".into();
        for prompt in settings.prompts() {
            let shared = review::shared_request(settings.prompt(), &prompt.text);
            assert!(shared.contains("{number}"), "the variables wait: {shared}");
            for (context, risk) in [
                (context("app", 1), Vec::new()),
                (risky.clone(), vec!["src/auth.ts".to_owned()]),
            ] {
                assert_eq!(
                    review::fill_request(&context, &risk, &shared),
                    review::request(&context, &prompt.text, &risk, settings.prompt()),
                );
            }
        }
    }

    #[test]
    fn the_summary_names_each_group() {
        let counts = Counts {
            started: 2,
            failed: 1,
            skipped: 1,
            ..Counts::default()
        };
        assert_eq!(counts.summary(), "2 started · 1 failed · 1 skipped");
    }

    /// The dialog for three pull requests (two ready, one from a fork),
    /// with the load already done and `launcher` standing in for T3.
    fn open(launcher: Launcher, cx: &mut TestAppContext) -> (Entity<BatchForm>, VisualTestContext) {
        let contexts = [context("app", 1), context("app", 2), {
            let mut fork = context("app", 3);
            fork.head_fork = Some("someone/app".into());
            fork
        }];
        let candidates: Vec<_> = contexts.iter().map(|c| of(c.clone(), &[])).collect();
        let settings = review::ReviewSettings::default();
        let loaded = Loaded {
            readiness: review::assess_batch(&candidates, &Ok(server()), &settings),
            contexts: contexts.iter().cloned().map(Some).collect(),
            providers: Some(server().providers),
        };
        let picked = contexts
            .iter()
            .map(|context| (context.pr.clone(), context.title.clone()))
            .collect();
        let (handle, form) = cx.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::init(cx);
            cx.set_global(Settings::default());
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        Default::default(),
                        size(px(640.), px(700.)),
                    ))),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    cx.new(|cx| {
                        let mut form = BatchForm::new(picked, launcher, window, cx);
                        form.apply(loaded, window, cx);
                        form
                    })
                },
            )
            .expect("open the window")
        });
        (form, VisualTestContext::from_window(handle, cx))
    }

    fn launches(form: &Entity<BatchForm>, cx: &mut VisualTestContext) -> Vec<Launch> {
        cx.update(|_, cx| {
            form.read(cx)
                .items
                .iter()
                .map(|item| item.launch.clone())
                .collect()
        })
    }

    #[gpui_kit::test]
    fn start_sends_one_thread_per_ready_pull_request_and_a_failure_stops_none(
        cx: &mut TestAppContext,
    ) {
        let sent: Arc<Mutex<Vec<ReviewLaunch>>> = Arc::default();
        let fail_two = Arc::new(Mutex::new(true));
        let launcher: Launcher = {
            let (sent, fail_two) = (sent.clone(), fail_two.clone());
            Arc::new(move |launch: &ReviewLaunch| {
                sent.lock().unwrap().push(launch.clone());
                if launch.branch == "review/pr-2" && *fail_two.lock().unwrap() {
                    return Err(Error::Transport("connection reset".into()));
                }
                Ok(LaunchedReview {
                    thread_id: format!("thread-{}", launch.branch),
                    link_error: None,
                })
            })
        };
        let (form, mut cx) = open(launcher, cx);
        form.update(&mut cx, |form, cx| form.send(cx));
        cx.run_until_parked();

        let mut first = sent.lock().unwrap().clone();
        first.sort_by(|a, b| a.branch.cmp(&b.branch));
        assert_eq!(
            first
                .iter()
                .map(|launch| (launch.branch.as_str(), launch.base_ref.as_str()))
                .collect::<Vec<_>>(),
            [("review/pr-1-2", "fix/1"), ("review/pr-2", "fix/2")],
            "one thread per ready pull request, none for the fork"
        );
        assert_ne!(first[0].request_id, first[1].request_id);
        assert!(first[0].message.contains("Revisar o PR #1 de acme/app"));
        assert!(first[1].message.contains("Revisar o PR #2 de acme/app"));
        assert!(
            first
                .iter()
                .all(|launch| launch.model.model == "claude-opus-5-5")
        );
        assert_eq!(
            launches(&form, &mut cx),
            [
                Launch::Started(None),
                Launch::Failed(
                    "T3 did not start the review: network error: connection reset".into()
                ),
                Launch::Idle,
            ]
        );
        let summary = cx.update(|_, cx| form.read(cx).counts().summary());
        assert_eq!(summary, "1 started · 1 failed · 1 skipped");

        *fail_two.lock().unwrap() = false;
        form.update(&mut cx, |form, cx| form.send(cx));
        cx.run_until_parked();
        let all = sent.lock().unwrap().clone();
        assert_eq!(all.len(), 3, "a retry sends only the failed one");
        assert_eq!(
            all[2].request_id, first[1].request_id,
            "under the same request id"
        );
        assert_eq!(
            launches(&form, &mut cx),
            [Launch::Started(None), Launch::Started(None), Launch::Idle]
        );
    }

    #[gpui_kit::test]
    fn the_edited_request_is_filled_for_each_pull_request(cx: &mut TestAppContext) {
        let sent: Arc<Mutex<Vec<ReviewLaunch>>> = Arc::default();
        let launcher: Launcher = {
            let sent = sent.clone();
            Arc::new(move |launch: &ReviewLaunch| {
                sent.lock().unwrap().push(launch.clone());
                Ok(LaunchedReview {
                    thread_id: "t".into(),
                    link_error: None,
                })
            })
        };
        let (form, mut cx) = open(launcher, cx);
        cx.update(|window, cx| {
            let text = form.read(cx).text.clone();
            text.update(cx, |state, cx| {
                state.set_value("Look at #{number} on {head_branch}.", window, cx)
            });
        });
        form.update(&mut cx, |form, cx| form.send(cx));
        cx.run_until_parked();
        let mut messages: Vec<String> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|launch| launch.message.clone())
            .collect();
        messages.sort();
        assert_eq!(messages, ["Look at #1 on fix/1.", "Look at #2 on fix/2."]);
    }
}
