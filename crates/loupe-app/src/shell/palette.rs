//! The Cmd+K command palette: routes, repositories, pinned teams, direct
//! jumps parsed from the query, and results merged from
//! `github::palette_search` and `linear::palette_search` as they arrive.
//!
//! A query is split into words. `pr:` (or `prs:`), `repo:` (or `repos:`),
//! `org:`, and `issue:` (or `issues:`) narrow what it lists; a team key such
//! as `ENG-` narrows the Linear issues to that team; the other words are the
//! text to search for. The field draws each prefix as a highlighted token.
//! Typed text also finds the pane commands (Split right, left, up, down),
//! listed first under Commands: text that names one is asking for it.
//!
//! With the pointer on an issue or a pull request, or one on screen, the
//! palette lists that item's commands first, the ones of its context menu
//! (`item_menu`): all of them while the query is empty, those whose name
//! holds the typed text after. A property's command (Change status…), and
//! its key on a row, turn the palette into that property's list, where
//! typing narrows the choices and Enter picks one.
//!
//! The list is gpui-component's `Command` inside a dialog, with local
//! filtering and its own search field turned off: this module owns the field
//! so it can mark the prefixes, and owns the matching because it ranks and
//! merges remote results that the stock label-only filter would hide.

use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::actions::{Cancel, Confirm};
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::input::{InlineToken, InlineTokenContext, Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::{ActiveTheme as _, Icon, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use loupe_github::{PrRef, RepoRef};
use loupe_linear::{IssueSummary, Team};

use crate::linear::issue_menu::{self, Field, FieldChoice};
use crate::route::Route;
use crate::services::Services;
use crate::shell::actions::PaletteOpenInNewTab;
use crate::shell::item_menu::{self, Command as ItemCommand, Item, Place};
use crate::shell::panes::Zone;
use crate::shell::{OpenOnClick as _, PaletteItem, navigate, open_in_new_tab, split_active_tab};
use crate::store::{Favorite, Settings};
use crate::theme::scale::dp;
use crate::theme::{scale, tokens};

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(250);
const PLACEHOLDER: &str =
    "Search, or narrow with pr:, repo:, org:, issue:, or a team key like ENG-";
/// Rows of the Repositories section.
const MAX_REPOS: usize = 8;
/// Repositories a `repo:` filter searches, one section each.
const MAX_REPO_SECTIONS: usize = 5;
/// Accounts a pull request search covers when the query names none.
const MAX_SCOPE_OWNERS: usize = 10;
const PULL_REQUEST_RESULTS: u32 = 10;
const REPO_PULL_REQUEST_RESULTS: u32 = 8;
const PREFIX_TOKEN_ID: &str = "palette-prefix";

/// Remembers the open palette so a second Cmd+K closes it.
struct PaletteSlot(Option<WeakEntity<PaletteView>>);

impl Global for PaletteSlot {}

/// The repositories the viewer reaches, fetched once per GitHub client.
struct ReachableRepos {
    client: Arc<loupe_github::GitHub>,
    repos: Vec<RepoRef>,
}

impl Global for ReachableRepos {}

fn is_open(window: &mut Window, cx: &mut App) -> bool {
    cx.try_global::<PaletteSlot>()
        .and_then(|slot| slot.0.as_ref())
        .is_some_and(|palette| palette.upgrade().is_some())
        && window.has_active_dialog(cx)
}

/// Opens the palette over `window`, or closes it when already open.
/// `target` is the item whose commands it lists first.
pub fn toggle(target: Option<(Item, Place)>, window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        window.close_dialog(cx);
        return;
    }
    open_palette(Mode::Search(target), window, cx);
}

/// Opens the palette on `field`'s list for `issue`, in place of whatever
/// it showed.
pub fn open_picker(issue: IssueSummary, field: Field, window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        window.close_dialog(cx);
    }
    open_palette(Mode::Pick(Box::new(issue), field), window, cx);
}

/// What the palette was opened for.
enum Mode {
    /// Search and go to, after the commands of the item, if any.
    Search(Option<(Item, Place)>),
    /// One property's list for an issue.
    Pick(Box<IssueSummary>, Field),
}

fn open_palette(mode: Mode, window: &mut Window, cx: &mut App) {
    let command = cx.new(|cx| CommandState::new(window, cx));
    let placeholder = match &mode {
        Mode::Search(_) => PLACEHOLDER.to_owned(),
        Mode::Pick(issue, field) => field.placeholder(&issue.identifier),
    };
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
    let view = cx.new(|cx| PaletteView::new(command, input.clone(), mode, window, cx));
    cx.set_global(PaletteSlot(Some(view.downgrade())));

    let focus_pending = std::cell::Cell::new(true);
    window.open_dialog(cx, move |dialog, window, cx| {
        if focus_pending.replace(false) {
            input.update(cx, |input, cx| input.focus(window, cx));
        }
        dialog
            .p_0()
            .close_button(false)
            .w(dp(620.))
            .margin_top(dp(96.))
            .on_close(|_, _, cx| cx.set_global(PaletteSlot(None)))
            .child(view.clone())
    });
}

/// Items a query names directly, without a network call: `owner/repo#123`
/// and PR URLs (via `PrRef::parse`) become `Route::PullRequest`, Linear
/// identifiers such as `ENG-123` become `Route::LinearIssue`, and a bare
/// `owner/repo` (or its GitHub URL) becomes `Route::Repo`.
pub fn direct_items(query: &str) -> Vec<PaletteItem> {
    let query = query.trim();
    let item = |title: String, subtitle: &str, route| PaletteItem {
        title: title.into(),
        subtitle: subtitle.to_owned().into(),
        route,
        comments: 0,
    };
    if let Some(pr) = PrRef::parse(query) {
        return vec![item(
            pr.to_string(),
            "Open pull request",
            Route::PullRequest(pr),
        )];
    }
    if let Some(identifier) = linear_identifier(query) {
        return vec![item(
            identifier.clone(),
            "Open Linear issue",
            Route::LinearIssue { identifier },
        )];
    }
    if let Some(repo) = repo_slug(query) {
        return vec![item(repo.to_string(), "Open repository", Route::Repo(repo))];
    }
    Vec::new()
}

/// `^[A-Za-z][A-Za-z0-9]{0,9}-\d+$`, uppercased.
fn linear_identifier(text: &str) -> Option<String> {
    let (key, number) = text.split_once('-')?;
    let number_is_valid = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    (is_team_key(key) && number_is_valid).then(|| format!("{}-{number}", key.to_ascii_uppercase()))
}

/// `^[A-Za-z][A-Za-z0-9]{0,9}$`: the shape of a Linear team key.
fn is_team_key(key: &str) -> bool {
    let mut chars = key.chars();
    let first_is_letter = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    first_is_letter && key.len() <= 10 && chars.all(|c| c.is_ascii_alphanumeric())
}

/// A word that is a team key and a dash (`eng-`), as the key, uppercased.
fn team_key_word(word: &str) -> Option<String> {
    word.strip_suffix('-')
        .filter(|key| is_team_key(key))
        .map(str::to_ascii_uppercase)
}

/// `owner/name` or `https://github.com/owner/name`, with GitHub's character
/// set so that `acme/web#x` and URLs of other sites do not pass.
fn repo_slug(text: &str) -> Option<RepoRef> {
    let slug = ["https://github.com/", "http://github.com/", "github.com/"]
        .iter()
        .find_map(|prefix| text.strip_prefix(prefix))
        .unwrap_or(text)
        .trim_end_matches('/');
    let (owner, name) = slug.split_once('/')?;
    let part_is_valid = |part: &str, extra: &[char]| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
    };
    (part_is_valid(owner, &['-']) && part_is_valid(name, &['-', '_', '.']))
        .then(|| RepoRef::new(owner, name))
}

/// A filter a word can start with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Prefix {
    PullRequest,
    Repo,
    Org,
    Issue,
}

/// Longer spellings first, so `prs:` is not read as `pr:` and an `s`.
const PREFIXES: [(&str, Prefix); 7] = [
    ("prs:", Prefix::PullRequest),
    ("pr:", Prefix::PullRequest),
    ("repos:", Prefix::Repo),
    ("repo:", Prefix::Repo),
    ("org:", Prefix::Org),
    ("issues:", Prefix::Issue),
    ("issue:", Prefix::Issue),
];

/// The prefix `word` starts with, in any case, and its length in bytes.
fn word_prefix(word: &str) -> Option<(Prefix, usize)> {
    PREFIXES.iter().find_map(|(spelling, prefix)| {
        word.get(..spelling.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(spelling))
            .then_some((*prefix, spelling.len()))
    })
}

/// Byte ranges of the prefixes that start a word of `text`: what the field
/// draws as tokens.
fn prefix_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut word_start = None;
    for (ix, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if !c.is_whitespace() {
            word_start.get_or_insert(ix);
            continue;
        }
        if let Some(start) = word_start.take()
            && let Some((_, len)) = word_prefix(&text[start..ix])
        {
            ranges.push(start..start + len);
        }
    }
    ranges
}

/// A typed query: the filters it sets and the words it searches for.
#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedQuery {
    /// Every prefix the query holds, even with nothing after it: `pr:` alone
    /// already means "only pull requests".
    prefixes: Vec<Prefix>,
    /// What follows `repo:`.
    repos: Vec<String>,
    /// What follows `org:`.
    orgs: Vec<String>,
    /// The key of an `ENG-` word, uppercased.
    team_key: Option<String>,
    /// The free words, and what follows `pr:` and `issue:`.
    words: Vec<String>,
}

impl ParsedQuery {
    fn parse(query: &str) -> Self {
        let mut parsed = Self::default();
        for word in query.split_whitespace() {
            if let Some((prefix, len)) = word_prefix(word) {
                if !parsed.prefixes.contains(&prefix) {
                    parsed.prefixes.push(prefix);
                }
                let value = &word[len..];
                if value.is_empty() {
                    continue;
                }
                match prefix {
                    Prefix::Repo => parsed.repos.push(value.to_owned()),
                    Prefix::Org => parsed.orgs.push(value.to_owned()),
                    Prefix::PullRequest | Prefix::Issue => parsed.words.push(value.to_owned()),
                }
            } else if let Some(key) = team_key_word(word) {
                parsed.team_key = Some(key);
            } else {
                parsed.words.push(word.to_owned());
            }
        }
        parsed
    }

    fn has(&self, prefix: Prefix) -> bool {
        self.prefixes.contains(&prefix)
    }

    fn text(&self) -> String {
        self.words.join(" ")
    }

    /// `pr:`, `repo:`, or `org:`: the query asks for GitHub.
    fn narrows_to_github(&self) -> bool {
        self.has(Prefix::PullRequest) || self.has(Prefix::Repo) || self.has(Prefix::Org)
    }

    /// `issue:` or a team key: the query asks for Linear.
    fn narrows_to_linear(&self) -> bool {
        self.has(Prefix::Issue) || self.team_key.is_some()
    }

    fn is_narrowed(&self) -> bool {
        self.narrows_to_github() || self.narrows_to_linear()
    }

    fn shows_github(&self) -> bool {
        self.narrows_to_github() || !self.narrows_to_linear()
    }

    fn shows_linear(&self) -> bool {
        self.narrows_to_linear() || !self.narrows_to_github()
    }
}

/// The repositories the filters match against: the watched ones first, then
/// the ones the viewer reaches, most recently pushed first.
#[derive(Clone, Debug, Default)]
struct RepoCatalog {
    watched: Vec<RepoRef>,
    reachable: Vec<RepoRef>,
}

impl RepoCatalog {
    fn all(&self) -> impl Iterator<Item = &RepoRef> {
        self.watched.iter().chain(
            self.reachable
                .iter()
                .filter(|repo| !self.watched.contains(repo)),
        )
    }
}

fn holds(repo: &RepoRef, needle: &str) -> bool {
    repo.to_string()
        .to_lowercase()
        .contains(&needle.to_lowercase())
}

/// The accounts `org:` names: each catalog owner that holds one of its
/// values, or the value as written when no owner does.
fn named_owners(parsed: &ParsedQuery, catalog: &RepoCatalog) -> Vec<String> {
    let mut owners: Vec<String> = Vec::new();
    let mut add = |owner: &str| {
        if !owners.iter().any(|known| known.eq_ignore_ascii_case(owner)) {
            owners.push(owner.to_owned());
        }
    };
    for value in &parsed.orgs {
        let needle = value.to_lowercase();
        let matching: Vec<&RepoRef> = catalog
            .all()
            .filter(|repo| repo.owner.to_lowercase().contains(&needle))
            .collect();
        if matching.is_empty() {
            add(value);
        }
        for repo in matching {
            add(&repo.owner);
        }
    }
    owners
}

fn owned_by(repo: &RepoRef, owners: &[String]) -> bool {
    owners.is_empty()
        || owners
            .iter()
            .any(|owner| repo.owner.eq_ignore_ascii_case(owner))
}

/// The repositories `repo:` names, at most five: those whose `owner/name`
/// holds one of its values, the ones named exactly first, inside the `org:`
/// accounts. A full `owner/name` the catalog lacks is taken as written.
fn named_repos(parsed: &ParsedQuery, catalog: &RepoCatalog) -> Vec<RepoRef> {
    let owners = named_owners(parsed, catalog);
    let exact = |repo: &RepoRef| {
        parsed.repos.iter().any(|value| {
            repo.name.eq_ignore_ascii_case(value) || repo.to_string().eq_ignore_ascii_case(value)
        })
    };
    let mut repos: Vec<RepoRef> = catalog
        .all()
        .filter(|repo| parsed.repos.iter().any(|value| holds(repo, value)))
        .filter(|repo| owned_by(repo, &owners))
        .cloned()
        .collect();
    for value in &parsed.repos {
        if let Some(repo) = repo_slug(value)
            && !repos.contains(&repo)
        {
            repos.push(repo);
        }
    }
    repos.sort_by_key(|repo| !exact(repo));
    repos.truncate(MAX_REPO_SECTIONS);
    repos
}

/// The accounts a pull request search covers when the query names none: the
/// owners of the catalog's repositories, watched first.
fn scope_owners(catalog: &RepoCatalog) -> Vec<String> {
    let mut owners: Vec<String> = Vec::new();
    for repo in catalog.all() {
        if owners.len() == MAX_SCOPE_OWNERS {
            break;
        }
        if !owners
            .iter()
            .any(|owner| owner.eq_ignore_ascii_case(&repo.owner))
        {
            owners.push(repo.owner.clone());
        }
    }
    owners
}

/// The Repositories rows: the watched repositories for an empty query;
/// otherwise each catalog repository that holds every `repo:` value and
/// every word, inside the `org:` accounts.
fn repo_rows(parsed: &ParsedQuery, catalog: &RepoCatalog) -> Vec<PaletteItem> {
    let row = |repo: &RepoRef| PaletteItem {
        title: repo.to_string().into(),
        subtitle: if catalog.watched.contains(repo) {
            "Watched repository"
        } else {
            "Repository"
        }
        .into(),
        route: Route::Repo(repo.clone()),
        comments: 0,
    };
    if !parsed.is_narrowed() && parsed.words.is_empty() {
        return catalog.watched.iter().take(MAX_REPOS).map(row).collect();
    }
    let owners = named_owners(parsed, catalog);
    catalog
        .all()
        .filter(|repo| {
            parsed
                .repos
                .iter()
                .chain(&parsed.words)
                .all(|needle| holds(repo, needle))
        })
        .filter(|repo| owned_by(repo, &owners))
        .take(MAX_REPOS)
        .map(row)
        .collect()
}

/// Where the results of a pull request search go.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PrTarget {
    /// The "Pull requests" section.
    All,
    /// The "Pull requests in owner/name" section.
    Repo(RepoRef),
}

/// One GitHub search the palette runs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PrSearch {
    target: PrTarget,
    query: String,
    first: u32,
}

/// `is:pr`, the text (or newest first without text), then the qualifiers.
fn search_query(text: &str, qualifiers: &[String]) -> String {
    let order = if text.is_empty() {
        "sort:updated-desc"
    } else {
        text
    };
    let mut query = format!("is:pr {order}");
    for qualifier in qualifiers {
        query.push(' ');
        query.push_str(qualifier);
    }
    query
}

/// The GitHub searches `parsed` asks for. With `repo:`, one per named
/// repository. Otherwise one over the `org:` accounts, or over the owners of
/// the catalog's repositories, or the pull requests that involve the viewer
/// when the catalog is empty; it runs only when there is text, `pr:`, or
/// `org:`.
fn pr_searches(parsed: &ParsedQuery, catalog: &RepoCatalog) -> Vec<PrSearch> {
    if !parsed.shows_github() {
        return Vec::new();
    }
    let text = parsed.text();
    if !parsed.repos.is_empty() {
        return named_repos(parsed, catalog)
            .into_iter()
            .map(|repo| PrSearch {
                query: search_query(&text, &[format!("repo:{repo}")]),
                target: PrTarget::Repo(repo),
                first: REPO_PULL_REQUEST_RESULTS,
            })
            .collect();
    }
    if text.is_empty() && !parsed.has(Prefix::PullRequest) && parsed.orgs.is_empty() {
        return Vec::new();
    }
    let owners = if parsed.orgs.is_empty() {
        scope_owners(catalog)
    } else {
        named_owners(parsed, catalog)
    };
    let scope: Vec<String> = if owners.is_empty() {
        vec!["involves:@me".to_owned()]
    } else {
        owners.iter().map(|owner| format!("user:{owner}")).collect()
    };
    vec![PrSearch {
        target: PrTarget::All,
        query: search_query(&text, &scope),
        first: PULL_REQUEST_RESULTS,
    }]
}

/// The Linear search `parsed` asks for: its text and the team key it is
/// narrowed to. None when it hides Linear or names nothing to search.
fn linear_search(parsed: &ParsedQuery) -> Option<(String, Option<String>)> {
    let text = parsed.text();
    (parsed.shows_linear() && (!text.is_empty() || parsed.team_key.is_some()))
        .then(|| (text, parsed.team_key.clone()))
}

/// Routes and starred teams and issues: what the palette lists, with the
/// watched repositories, before the user types anything.
fn local_items(cx: &App) -> Vec<PaletteItem> {
    let item = |title: &str, subtitle: &str, route: Route| PaletteItem {
        title: title.to_owned().into(),
        subtitle: subtitle.to_owned().into(),
        route,
        comments: 0,
    };
    let settings = Settings::get(cx);
    let mut items = vec![
        item("Inbox", "GitHub notifications", Route::Notifications),
        item("Reviews", "Pull requests waiting on you", Route::Inbox),
        item("My issues", "Linear", Route::LinearMyIssues),
        item(
            "Projects",
            "Linear",
            Route::LinearProjects { team_id: None },
        ),
        item("Views", "Linear", Route::LinearViews { team_id: None }),
        item("Settings", "Accounts and preferences", Route::Settings),
    ];
    items.extend(
        settings
            .favorites
            .iter()
            .filter_map(|favorite| match favorite {
                Favorite::Team { team_id, name } => Some(item(
                    name,
                    "Favorite team",
                    Route::LinearTeam {
                        team_id: team_id.clone(),
                        name: name.clone(),
                        view_id: None,
                    },
                )),
                Favorite::Issue { identifier, title } => Some(item(
                    title,
                    identifier,
                    Route::LinearIssue {
                        identifier: identifier.clone(),
                    },
                )),
                Favorite::View { .. } => None,
            }),
    );
    items
}

/// The headings of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Group {
    JumpTo,
    GoTo,
    Repositories,
    Teams,
    PullRequests,
    RepoPullRequests(RepoRef),
    LinearIssues,
}

impl Group {
    fn heading(&self) -> SharedString {
        match self {
            Group::JumpTo => "Jump to".into(),
            Group::GoTo => "Go to".into(),
            Group::Repositories => "Repositories".into(),
            Group::Teams => "Teams".into(),
            Group::PullRequests => "Pull requests".into(),
            Group::RepoPullRequests(repo) => format!("Pull requests in {repo}").into(),
            Group::LinearIssues => "Linear issues".into(),
        }
    }

    /// Display order; the sections of one rank keep the order they came in.
    fn rank(&self) -> u8 {
        match self {
            Group::JumpTo => 0,
            Group::GoTo => 1,
            Group::Repositories => 2,
            Group::Teams => 3,
            Group::PullRequests => 4,
            Group::RepoPullRequests(_) => 5,
            Group::LinearIssues => 6,
        }
    }

    fn of(route: &Route) -> Group {
        match route {
            Route::Inbox
            | Route::Notifications
            | Route::LinearMyIssues
            | Route::LinearProjects { .. }
            | Route::LinearProject { .. }
            | Route::LinearViews { .. }
            | Route::Settings => Group::GoTo,
            Route::Repo(_) | Route::Actions { .. } => Group::Repositories,
            Route::LinearTeam { .. } => Group::Teams,
            Route::PullRequest(_) | Route::WorkflowRun { .. } => Group::PullRequests,
            Route::LinearIssue { .. } => Group::LinearIssues,
        }
    }
}

/// 0 for a substring match, 1 for a subsequence match, `None` for no match.
/// Case-insensitive, over the title and the subtitle.
fn match_rank(query: &str, item: &PaletteItem) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let haystack = format!("{} {}", item.title, item.subtitle).to_lowercase();
    if haystack.contains(&query) {
        return Some(0);
    }
    let mut wanted = query.chars().filter(|c| !c.is_whitespace()).peekable();
    for c in haystack.chars() {
        if wanted.peek() == Some(&c) {
            wanted.next();
        }
    }
    wanted.peek().is_none().then_some(1)
}

/// Which search a remote answer belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    GitHub(PrTarget),
    Linear,
}

/// Results of the remote searches for the latest query. Every new query bumps
/// `generation`, so a slow answer to an older query is dropped.
#[derive(Default)]
struct RemoteResults {
    generation: u64,
    pull_requests: Vec<PaletteItem>,
    /// One entry per `repo:` search, in the order they started.
    repo_pull_requests: Vec<(RepoRef, Vec<PaletteItem>)>,
    linear: Vec<PaletteItem>,
    awaiting: usize,
}

impl RemoteResults {
    /// Starts a new query: forgets old results and returns its generation.
    fn reset(&mut self) -> u64 {
        self.generation += 1;
        self.pull_requests.clear();
        self.repo_pull_requests.clear();
        self.linear.clear();
        self.awaiting = 0;
        self.generation
    }

    /// Records that searches for `sources` started for `generation`. False
    /// when the query has moved on.
    fn expect(&mut self, generation: u64, sources: &[Source]) -> bool {
        if generation != self.generation {
            return false;
        }
        for source in sources {
            if let Source::GitHub(PrTarget::Repo(repo)) = source {
                self.repo_pull_requests.push((repo.clone(), Vec::new()));
            }
        }
        self.awaiting += sources.len();
        true
    }

    /// Stores an answer. False, and nothing stored, when it is stale.
    fn accept(&mut self, source: &Source, items: Vec<PaletteItem>, generation: u64) -> bool {
        if generation != self.generation {
            return false;
        }
        match source {
            Source::GitHub(PrTarget::All) => self.pull_requests = items,
            Source::GitHub(PrTarget::Repo(repo)) => {
                if let Some((_, slot)) = self
                    .repo_pull_requests
                    .iter_mut()
                    .find(|(of, _)| of == repo)
                {
                    *slot = items;
                }
            }
            Source::Linear => self.linear = items,
        }
        self.awaiting = self.awaiting.saturating_sub(1);
        true
    }

    fn is_loading(&self) -> bool {
        self.awaiting > 0
    }
}

/// The commands the palette runs on the panes, with their keys.
const PANE_COMMANDS: [(&str, Zone, Option<&str>); 4] = [
    ("Split right", Zone::Right, Some("cmd-\\")),
    ("Split left", Zone::Left, None),
    ("Split up", Zone::Up, None),
    ("Split down", Zone::Down, Some("cmd-shift-\\")),
];

/// The pane commands typed text names: it starts one's label and holds
/// at least "spl", so a short query for a page ("li", "do") never puts a
/// split under Enter. None for a narrowed query, which lists pages.
fn pane_commands(parsed: &ParsedQuery) -> Vec<(&'static str, Zone, Option<&'static str>)> {
    let text = parsed.text().trim().to_lowercase();
    if parsed.is_narrowed() || text.len() < 3 {
        return Vec::new();
    }
    PANE_COMMANDS
        .into_iter()
        .filter(|(label, _, _)| label.to_lowercase().starts_with(&text))
        .collect()
}

/// A row listed before the pages: a command on the panes or on the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Leading {
    Pane(Zone),
    Item(ItemCommand),
}

/// The commands of `target` a query asks for: all of them while it is
/// empty, else those whose palette name holds its text (two letters at
/// least). None for a narrowed query, which lists pages.
fn item_commands(
    target: Option<&(Item, Place)>,
    t3: bool,
    parsed: &ParsedQuery,
) -> Vec<ItemCommand> {
    let Some((item, place)) = target else {
        return Vec::new();
    };
    let text = parsed.text().trim().to_lowercase();
    if parsed.is_narrowed() || text.chars().count() == 1 {
        return Vec::new();
    }
    item_menu::sections(item, *place, t3)
        .into_iter()
        .flatten()
        .filter(|command| {
            text.is_empty()
                || command
                    .palette_label(item)
                    .to_lowercase()
                    .contains(text.as_str())
        })
        .collect()
}

/// The choices of an open list whose name holds `query`, in the list's
/// order.
fn matching_choices(choices: &[FieldChoice], query: &str) -> Vec<FieldChoice> {
    let query = query.trim().to_lowercase();
    choices
        .iter()
        .filter(|choice| query.is_empty() || choice.name().to_lowercase().contains(&query))
        .cloned()
        .collect()
}

/// The headings and rows for a query, direct jumps first. A route appears
/// once, under the first section that offers it.
fn build_sections(
    parsed: &ParsedQuery,
    direct: &[PaletteItem],
    local: &[PaletteItem],
    catalog: &RepoCatalog,
    remote: &RemoteResults,
) -> Vec<(Group, Vec<PaletteItem>)> {
    let mut candidates: Vec<(Group, PaletteItem)> = direct
        .iter()
        .map(|item| (Group::JumpTo, item.clone()))
        .collect();
    if !parsed.is_narrowed() {
        let text = parsed.text();
        let mut ranked: Vec<(u8, &PaletteItem)> = local
            .iter()
            .filter_map(|item| match_rank(&text, item).map(|rank| (rank, item)))
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        candidates.extend(
            ranked
                .into_iter()
                .map(|(_, item)| (Group::of(&item.route), item.clone())),
        );
    }
    if parsed.shows_github() {
        if !parsed.has(Prefix::PullRequest) {
            candidates.extend(
                repo_rows(parsed, catalog)
                    .into_iter()
                    .map(|item| (Group::Repositories, item)),
            );
        }
        candidates.extend(
            remote
                .pull_requests
                .iter()
                .map(|item| (Group::PullRequests, item.clone())),
        );
        for (repo, items) in &remote.repo_pull_requests {
            let group = Group::RepoPullRequests(repo.clone());
            candidates.extend(items.iter().map(|item| (group.clone(), item.clone())));
        }
    }
    if parsed.shows_linear() {
        candidates.extend(
            remote
                .linear
                .iter()
                .map(|item| (Group::LinearIssues, item.clone())),
        );
    }

    let mut seen: Vec<Route> = Vec::new();
    let mut sections: Vec<(Group, Vec<PaletteItem>)> = Vec::new();
    for (group, item) in candidates {
        if seen.contains(&item.route) {
            continue;
        }
        seen.push(item.route.clone());
        match sections.iter_mut().find(|(of, _)| *of == group) {
            Some((_, items)) => items.push(item),
            None => sections.push((group, vec![item])),
        }
    }
    sections.sort_by_key(|(group, _)| group.rank());
    sections
}

struct PaletteView {
    command: Entity<CommandState>,
    input: Entity<InputState>,
    local: Vec<PaletteItem>,
    catalog: RepoCatalog,
    /// False until the reachable repositories arrive. GitHub searches wait
    /// for them, because they decide what every search covers.
    catalog_ready: bool,
    /// The generation whose GitHub searches wait for the catalog.
    github_waiting: Option<u64>,
    query: String,
    parsed: ParsedQuery,
    direct: Vec<PaletteItem>,
    remote: RemoteResults,
    /// The debounce timer and the searches in flight; dropping them cancels.
    pending: Vec<Task<()>>,
    /// True from a keystroke until its debounce ends: a search is coming.
    debouncing: bool,
    _catalog_load: Option<Task<()>>,
    /// Routes of the rows the list shows now, by section and row, so that a
    /// confirm resolves against what the user saw even if results land later.
    shown: Vec<Vec<Route>>,
    /// The sections of commands listed before `shown`'s: the item's, then
    /// the panes'.
    leading: Vec<Vec<Leading>>,
    /// The issue or pull request whose commands come first.
    target: Option<(Item, Place)>,
    /// A T3 thread is linked to the target pull request.
    target_t3: bool,
    /// The property list the palette shows instead of the search.
    picker: Option<Picker>,
    /// Set by Cmd+Enter just before it confirms: open the result in a new
    /// background tab.
    new_tab: bool,
    _input_events: Subscription,
}

/// One property's list for an issue.
struct Picker {
    issue: IssueSummary,
    field: Field,
    /// The issue's team catalog, which holds the choices.
    team: Option<Result<Arc<Team>, SharedString>>,
    /// The choices the list shows now, so a confirm picks what was seen.
    shown: Vec<FieldChoice>,
    _load: Option<Task<()>>,
}

impl Picker {
    fn choices(&self) -> Vec<FieldChoice> {
        match &self.team {
            Some(Ok(team)) => issue_menu::choices(self.field, team, &self.issue),
            _ => Vec::new(),
        }
    }
}

impl PaletteView {
    fn new(
        command: Entity<CommandState>,
        input: Entity<InputState>,
        mode: Mode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input_events = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::Change) {
                this.on_input_changed(window, cx);
            }
        });
        let mut view = Self {
            command,
            input,
            local: local_items(cx),
            catalog: RepoCatalog {
                watched: Settings::get(cx).watched_repos.clone(),
                reachable: Vec::new(),
            },
            catalog_ready: false,
            github_waiting: None,
            query: String::new(),
            parsed: ParsedQuery::default(),
            direct: Vec::new(),
            remote: RemoteResults::default(),
            pending: Vec::new(),
            debouncing: false,
            _catalog_load: None,
            shown: Vec::new(),
            leading: Vec::new(),
            target: None,
            target_t3: false,
            picker: None,
            new_tab: false,
            _input_events: input_events,
        };
        match mode {
            Mode::Search(target) => {
                view.set_target(target, cx);
                view.load_catalog(window, cx);
            }
            Mode::Pick(issue, field) => view.open_list(*issue, field, cx),
        }
        view
    }

    /// Lists `target`'s commands first, and the T3 row once T3 names a
    /// thread linked to the pull request.
    fn set_target(&mut self, target: Option<(Item, Place)>, cx: &mut Context<Self>) {
        if let Some((Item::PullRequest(pull), _)) = &target {
            self.target_t3 = item_menu::t3_link(&pull.pr, cx).is_some();
            let lookup = item_menu::find_t3_link(&pull.pr, cx);
            self.pending.push(cx.spawn(async move |this, cx| {
                let found = lookup.await.is_some();
                this.update(cx, |this, cx| {
                    if this.target_t3 != found {
                        this.target_t3 = found;
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        self.target = target;
    }

    /// Shows `field`'s list for `issue`, from the team catalog once it is
    /// there.
    fn open_list(&mut self, issue: IssueSummary, field: Field, cx: &mut Context<Self>) {
        let issue = issue_menu::freshest(&issue, cx);
        let team = issue_menu::cached_team(&issue, cx);
        let load = team.is_none().then(|| {
            let load = issue_menu::load_team(&issue, cx);
            cx.spawn(async move |this, cx| {
                let team = load.await;
                this.update(cx, |this, cx| {
                    if let Some(picker) = &mut this.picker {
                        picker.team = Some(team);
                        cx.notify();
                    }
                })
                .ok();
            })
        });
        self.picker = Some(Picker {
            issue,
            field,
            team: team.map(Ok),
            shown: Vec::new(),
            _load: load,
        });
    }

    /// Fills `catalog.reachable` from the cache, or from GitHub once per
    /// client.
    fn load_catalog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(client) = Services::get(cx).github.clone() else {
            self.catalog_ready = true;
            return;
        };
        if let Some(cached) = cx
            .try_global::<ReachableRepos>()
            .filter(|cached| Arc::ptr_eq(&cached.client, &client))
        {
            self.catalog.reachable = cached.repos.clone();
            self.catalog_ready = true;
            return;
        }
        let fetch = crate::github::palette_repositories(cx);
        self._catalog_load = Some(cx.spawn_in(window, async move |this, cx| {
            let repos = fetch.await;
            this.update_in(cx, |this, window, cx| {
                if !repos.is_empty() {
                    cx.set_global(ReachableRepos {
                        client,
                        repos: repos.clone(),
                    });
                }
                this.catalog.reachable = repos;
                this.catalog_ready = true;
                if let Some(generation) = this.github_waiting.take() {
                    this.start_github(generation, window, cx);
                }
                this.sync_loading(window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn on_input_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            let text = self.input.read(cx).value();
            self.query = text.trim().to_owned();
            self.command
                .update(cx, |command, cx| command.set_query(text, window, cx));
            cx.notify();
            return;
        }
        mark_prefixes(&self.input, window, cx);
        let text = self.input.read(cx).value();
        if text.trim() == self.query {
            return;
        }
        // The list's own query is hidden; setting it moves the highlight back
        // to the first row, as typing in a stock palette does.
        self.command.update(cx, |command, cx| {
            command.set_query(text.clone(), window, cx)
        });
        self.set_query(&text, window, cx);
    }

    fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.query = query.trim().to_owned();
        self.parsed = ParsedQuery::parse(&self.query);
        self.direct = direct_items(&self.query);
        self.pending.clear();
        self.github_waiting = None;
        self.debouncing = !self.query.is_empty();
        let generation = self.remote.reset();
        self.sync_loading(window, cx);
        cx.notify();
        if self.query.is_empty() {
            return;
        }

        let executor = cx.background_executor().clone();
        let debounce = cx.spawn_in(window, async move |this, cx| {
            executor.timer(SEARCH_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| {
                this.debouncing = false;
                this.start_linear(generation, window, cx);
                if this.catalog_ready {
                    this.start_github(generation, window, cx);
                } else {
                    this.github_waiting = Some(generation);
                }
                this.sync_loading(window, cx);
                cx.notify();
            })
            .ok();
        });
        self.pending.push(debounce);
    }

    fn start_linear(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some((text, team_key)) = linear_search(&self.parsed) else {
            return;
        };
        if !self.remote.expect(generation, &[Source::Linear]) {
            return;
        }
        let search = crate::linear::palette_search(&text, team_key.as_deref(), cx);
        self.track(Source::Linear, search, generation, window, cx);
    }

    fn start_github(&mut self, generation: u64, window: &mut Window, cx: &mut Context<Self>) {
        let searches = pr_searches(&self.parsed, &self.catalog);
        let sources: Vec<Source> = searches
            .iter()
            .map(|search| Source::GitHub(search.target.clone()))
            .collect();
        if !self.remote.expect(generation, &sources) {
            return;
        }
        for (search, source) in searches.into_iter().zip(sources) {
            let task = crate::github::palette_search(&search.query, search.first, cx);
            self.track(source, task, generation, window, cx);
        }
    }

    /// Stores `search`'s answer when it lands, unless the query moved on.
    fn track(
        &mut self,
        source: Source,
        search: Task<Vec<PaletteItem>>,
        generation: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_loading(window, cx);
        let task = cx.spawn_in(window, async move |this, cx| {
            let items = search.await;
            this.update_in(cx, |this, window, cx| {
                if this.remote.accept(&source, items, generation) {
                    this.sync_loading(window, cx);
                    cx.notify();
                }
            })
            .ok();
        });
        self.pending.push(task);
    }

    fn is_searching(&self) -> bool {
        self.debouncing || self.remote.is_loading() || self.github_waiting.is_some()
    }

    /// Spins the field while a search runs. The list itself is never told it
    /// is loading: a loading `Command` with no rows draws nothing at all, so
    /// the palette shrank to a gap. Its empty slot shows the status instead.
    fn sync_loading(&self, window: &mut Window, cx: &mut Context<Self>) {
        let loading = self.is_searching();
        self.input
            .update(cx, |input, cx| input.set_loading(loading, window, cx));
    }

    fn route_at(&self, section: usize, row: usize) -> Option<Route> {
        let section = section.checked_sub(self.leading.len())?;
        self.shown.get(section)?.get(row).cloned()
    }

    /// The command at `row` of a section that comes before the pages.
    fn command_at(&self, section: usize, row: usize) -> Option<Leading> {
        self.leading.get(section)?.get(row).copied()
    }
}

/// Turns each prefix that starts a word into a token, which the field draws
/// highlighted. Replacing a range with a token leaves the caret where it was.
fn mark_prefixes(input: &Entity<InputState>, window: &mut Window, cx: &mut App) {
    input.update(cx, |input, cx| {
        let text = input.value().to_string();
        let missing: Vec<Range<usize>> = prefix_ranges(&text)
            .into_iter()
            .filter(|range| !input.tokens().iter().any(|span| span.range() == *range))
            .collect();
        for range in missing {
            let token = InlineToken::new(PREFIX_TOKEN_ID, text[range.clone()].to_owned());
            input
                .replace_range_with_token(range, token, window, cx)
                .ok();
        }
    });
}

/// A prefix in the field: the accent's wash behind the accent's text.
fn prefix_token(token: &InlineTokenContext, _: &mut Window, _: &mut App) -> Div {
    div()
        .flex()
        .items_center()
        .h(token.line_height())
        .px_0p5()
        .rounded(dp(4.))
        .bg(if token.is_selected() {
            tokens::accent().opacity(0.32)
        } else {
            tokens::accent().opacity(0.16)
        })
        .text_color(tokens::link())
        .child(token.token().label().clone())
}

/// The icon a route shows in the palette, the same one the sidebar uses.
fn route_icon(route: &Route) -> IconName {
    match route {
        Route::Inbox => IconName::GitPullRequest,
        Route::Notifications => IconName::Inbox,
        Route::Repo(_) => IconName::FolderGit2,
        Route::PullRequest(_) => IconName::GitPullRequest,
        Route::Actions { .. } | Route::WorkflowRun { .. } => IconName::Workflow,
        Route::LinearMyIssues | Route::LinearIssue { .. } => IconName::CircleDot,
        Route::LinearTeam { .. } => IconName::Users,
        Route::LinearProjects { .. } | Route::LinearProject { .. } => IconName::Box,
        Route::LinearViews { .. } => IconName::Layers,
        Route::Settings => IconName::Settings,
    }
}

fn row(item: PaletteItem) -> CommandItem {
    let PaletteItem {
        title,
        subtitle,
        route,
        comments,
    } = item;
    let icon = route_icon(&route);
    let id = SharedString::from(format!("palette-result-{route:?}"));
    CommandItem::new().label(title.clone()).child(move |_, cx| {
        let muted = cx.theme().muted_foreground;
        let route = route.clone();
        let selector = id.to_string();
        h_flex()
            .id(id.clone())
            .debug_selector(|| selector)
            .on_middle_click(move |_, window, cx| {
                window.close_dialog(cx);
                open_in_new_tab(route.clone(), window, cx);
            })
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                svg()
                    .path(icon.path())
                    .flex_none()
                    .size_4()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .child(title.clone()),
            )
            .when(!subtitle.is_empty(), |row| {
                row.child(
                    div()
                        .flex_none()
                        .max_w(relative(0.45))
                        .truncate()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(subtitle.clone()),
                )
            })
            .when(comments > 0, |row| {
                row.child(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .text_size(scale::meta())
                        .text_color(muted)
                        .child(
                            svg()
                                .path(IconName::MessageSquare.path())
                                .size_3()
                                .text_color(muted),
                        )
                        .child(comments.to_string()),
                )
            })
    })
}

fn command_row(label: &'static str, zone: Zone, keys: Option<&'static str>) -> CommandItem {
    let icon = match zone {
        Zone::Left | Zone::Right | Zone::Center => IconName::Columns2,
        Zone::Up | Zone::Down => IconName::Rows2,
    };
    CommandItem::new().label(label).child(move |_, cx| {
        let muted = cx.theme().muted_foreground;
        h_flex()
            .id(SharedString::from(format!("palette-command-{zone:?}")))
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                svg()
                    .path(icon.path())
                    .flex_none()
                    .size_4()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .child(label),
            )
            .when_some(
                keys.and_then(|keys| Keystroke::parse(keys).ok()),
                |row, keys| row.child(Kbd::new(keys).outline()),
            )
    })
}

/// The key the menus show for `action`: the last one bound.
fn shown_key(action: &dyn Action, cx: &App) -> Option<Keystroke> {
    let keymap = cx.key_bindings();
    let keymap = keymap.borrow();
    let binding = keymap.bindings_for_action(action).last()?;
    binding
        .keystrokes()
        .first()
        .map(|key| key.as_keystroke().clone())
}

/// A command of the item the palette was opened on: its icon, its palette
/// name, and its key.
fn item_command_row(command: ItemCommand, item: &Item, cx: &App) -> CommandItem {
    let label = command.palette_label(item);
    let icon = command.icon(item);
    let key = command
        .action()
        .and_then(|action| shown_key(action.as_ref(), cx));
    let id = SharedString::from(format!("palette-item-{command:?}"));
    CommandItem::new().label(label).child(move |_, cx| {
        let muted = cx.theme().muted_foreground;
        let selector = id.to_string();
        h_flex()
            .id(id.clone())
            .debug_selector(|| selector)
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(
                svg()
                    .path(icon.path())
                    .flex_none()
                    .size_4()
                    .text_color(muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .child(label),
            )
            .when_some(key.clone(), |row, key| row.child(Kbd::new(key).outline()))
    })
}

/// A choice of an open property list: its mark, its name, and a check on
/// the issue's value.
fn choice_row(choice: FieldChoice) -> CommandItem {
    let name = SharedString::from(choice.name().to_owned());
    let id = SharedString::from(format!("palette-choice-{}", choice.id()));
    CommandItem::new().label(name.clone()).child(move |_, cx| {
        let selector = id.to_string();
        let accessible = if choice.selected() {
            format!("{name}, selected")
        } else {
            name.to_string()
        };
        h_flex()
            .id(id.clone())
            .debug_selector(|| selector)
            .aria_label(accessible)
            .flex_1()
            .min_w_0()
            .gap_2()
            .child(choice.mark(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(scale::body())
                    .child(name.clone()),
            )
            .when(choice.selected(), |row| {
                row.child(
                    svg()
                        .path(IconName::Check.path())
                        .flex_none()
                        .size_4()
                        .text_color(cx.theme().foreground),
                )
            })
    })
}

/// What the status row says while the searches for `parsed` run.
fn searching_label(parsed: &ParsedQuery) -> &'static str {
    match (parsed.shows_github(), parsed.shows_linear()) {
        (true, false) => "Searching pull requests…",
        (false, true) => "Searching Linear issues…",
        _ => "Searching…",
    }
}

/// The one row the list holds when it has no result: what is going on, with
/// the frame of a result row (and an empty icon slot, so the text starts
/// where the titles do), so the palette keeps its height and spacing.
fn status_row(label: impl Into<SharedString>, muted: Hsla) -> impl IntoElement {
    let label = label.into();
    h_flex()
        .id("palette-status")
        .debug_selector(|| "palette-status".into())
        .role(Role::Status)
        .aria_label(label.clone())
        .w_full()
        .gap_2()
        .px_2()
        .py_1p5()
        .text_sm()
        .text_color(muted)
        .child(div().flex_none().size_4())
        .child(div().text_size(scale::body()).child(label))
}

/// A key hint in the footer: the key as `Kbd`, then what it does.
fn hint(keys: &'static str, action: &'static str, muted: Hsla) -> impl IntoElement {
    h_flex()
        .gap_1()
        .text_size(scale::meta())
        .text_color(muted)
        .when_some(Keystroke::parse(keys).ok(), |row, keys| {
            row.child(Kbd::new(keys).outline())
        })
        .child(div().child(action))
}

impl PaletteView {
    /// One property's list: its choices with their marks and a check on the
    /// current one, narrowed by the field. Enter picks the row.
    fn render_list(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let Some(picker) = self.picker.as_mut() else {
            return div().into_any_element();
        };
        let shown = matching_choices(&picker.choices(), &self.query);
        picker.shown = shown.clone();
        let status: SharedString = match &picker.team {
            None => "Loading…".into(),
            Some(Err(message)) => message.clone(),
            Some(Ok(_)) => "No results".into(),
        };
        let heading = format!("{} · {}", picker.issue.identifier, picker.field.label());
        let view = cx.entity().downgrade();
        let input = self.input.clone();
        Command::new(&self.command)
            .searchable(false)
            .filterable(false)
            .bordered(false)
            .max_h(dp(380.))
            .header(move |_, _, cx| field_header(&input, false, cx))
            .empty(move |_, _, cx| status_row(status.clone(), cx.theme().muted_foreground))
            .footer(|_, _, cx| {
                let muted = cx.theme().muted_foreground;
                footer(cx)
                    .child(hint("up", "Move", muted))
                    .child(hint("enter", "Select", muted))
                    .child(hint("escape", "Close", muted))
            })
            .on_confirm(move |path, window, cx| {
                let picked = view
                    .read_with(cx, |this, _| {
                        let picker = this.picker.as_ref()?;
                        let choice = picker.shown.get(path.row)?.clone();
                        let team = picker.team.clone()?.ok()?;
                        Some((picker.issue.clone(), choice, team))
                    })
                    .ok()
                    .flatten();
                if let Some((issue, choice, team)) = picked {
                    window.close_dialog(cx);
                    issue_menu::pick(&issue, &choice, team, window, cx);
                }
            })
            .group(
                CommandGroup::new()
                    .label(heading)
                    .items(shown.into_iter().map(choice_row)),
            )
            .into_any_element()
    }
}

/// The field over the list, with the prefixes drawn as tokens while
/// searching.
fn field_header(input: &Entity<InputState>, tokens: bool, cx: &App) -> Div {
    let field = Input::new(input)
        .prefix(
            Icon::new(gpui_kit::component::IconName::Search)
                .text_color(cx.theme().muted_foreground),
        )
        .appearance(false)
        .p_0();
    div()
        .flex_none()
        .px_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .child(if tokens {
            field.token(prefix_token)
        } else {
            field
        })
}

fn footer(cx: &App) -> Div {
    h_flex()
        .gap_4()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(cx.theme().border)
}

impl Render for PaletteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let body = if self.picker.is_some() {
            self.render_list(cx)
        } else {
            self.render_search(cx)
        };
        div()
            .on_action(cx.listener(|this, _: &PaletteOpenInNewTab, window, cx| {
                this.new_tab = true;
                window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
            }))
            // Escape clears a query first and closes the palette only once
            // the field is empty, as the stock search field does.
            .capture_action(cx.listener(|this, _: &Cancel, window, cx| {
                if this.input.read(cx).value().is_empty() {
                    return;
                }
                this.input
                    .update(cx, |input, cx| input.set_value("", window, cx));
                this.on_input_changed(window, cx);
                cx.stop_propagation();
            }))
            .child(body)
    }
}

impl PaletteView {
    fn render_search(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sections = build_sections(
            &self.parsed,
            &self.direct,
            &self.local,
            &self.catalog,
            &self.remote,
        );
        self.shown = sections
            .iter()
            .map(|(_, items)| items.iter().map(|item| item.route.clone()).collect())
            .collect();
        let item_commands = item_commands(self.target.as_ref(), self.target_t3, &self.parsed);
        let pane_commands = pane_commands(&self.parsed);
        self.leading = [
            item_commands
                .iter()
                .map(|command| Leading::Item(*command))
                .collect::<Vec<_>>(),
            pane_commands
                .iter()
                .map(|(_, zone, _)| Leading::Pane(*zone))
                .collect(),
        ]
        .into_iter()
        .filter(|section| !section.is_empty())
        .collect();

        let view = cx.entity().downgrade();
        let input = self.input.clone();
        let status = if self.is_searching() {
            searching_label(&self.parsed)
        } else {
            "No results"
        };
        let mut command = Command::new(&self.command)
            .searchable(false)
            .filterable(false)
            .bordered(false)
            .max_h(dp(380.))
            .header(move |_, _, cx| field_header(&input, true, cx))
            .empty(move |_, _, cx| status_row(status, cx.theme().muted_foreground))
            .footer(|_, _, cx| {
                let muted = cx.theme().muted_foreground;
                footer(cx)
                    .child(hint("up", "Move", muted))
                    .child(hint("enter", "Open", muted))
                    .child(hint("cmd-enter", "New tab", muted))
                    .child(hint("escape", "Close", muted))
            })
            .on_confirm(move |path, window, cx| {
                let command = view
                    .read_with(cx, |this, _| {
                        this.command_at(path.section, path.row)
                            .map(|command| (command, this.target.clone()))
                    })
                    .ok()
                    .flatten();
                match command {
                    Some((Leading::Pane(zone), _)) => {
                        window.close_dialog(cx);
                        split_active_tab(zone, window, cx);
                        return;
                    }
                    Some((Leading::Item(command), Some((item, _)))) => {
                        window.close_dialog(cx);
                        item_menu::run(command, &item, window, cx);
                        return;
                    }
                    _ => {}
                }
                let picked = view
                    .update(cx, |this, _| {
                        let new_tab = std::mem::take(&mut this.new_tab);
                        this.route_at(path.section, path.row)
                            .map(|route| (route, new_tab))
                    })
                    .ok()
                    .flatten();
                if let Some((route, new_tab)) = picked {
                    window.close_dialog(cx);
                    if new_tab || window.modifiers().platform {
                        open_in_new_tab(route, window, cx);
                    } else {
                        navigate(route, window, cx);
                    }
                }
            });
        if let Some((item, _)) = self.target.as_ref().filter(|_| !item_commands.is_empty()) {
            command = command.group(
                CommandGroup::new().label(item.reference()).items(
                    item_commands
                        .into_iter()
                        .map(|command| item_command_row(command, item, cx)),
                ),
            );
        }
        if !pane_commands.is_empty() {
            command = command.group(
                CommandGroup::new().label("Commands").items(
                    pane_commands
                        .into_iter()
                        .map(|(label, zone, keys)| command_row(label, zone, keys)),
                ),
            );
        }
        for (group, items) in sections {
            command = command.group(
                CommandGroup::new()
                    .label(group.heading())
                    .items(items.into_iter().map(row)),
            );
        }
        command.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Group, ParsedQuery, PrSearch, PrTarget, Prefix, RemoteResults, RepoCatalog, Source,
        build_sections, direct_items, linear_search, match_rank, pane_commands, pr_searches,
        prefix_ranges, repo_rows, searching_label,
    };
    use crate::route::Route;
    use crate::shell::PaletteItem;
    use loupe_github::{PrRef, RepoRef};

    fn item(title: &str, subtitle: &str, route: Route) -> PaletteItem {
        PaletteItem {
            title: title.to_owned().into(),
            subtitle: subtitle.to_owned().into(),
            route,
            comments: 0,
        }
    }

    fn pr_item(owner: &str, name: &str, number: u64) -> PaletteItem {
        let pr = PrRef::new(RepoRef::new(owner, name), number);
        item("Fix", &pr.to_string(), Route::PullRequest(pr))
    }

    fn only_route(query: &str) -> Route {
        let mut items = direct_items(query);
        assert_eq!(items.len(), 1, "{query:?} should give one item");
        items.remove(0).route
    }

    fn catalog() -> RepoCatalog {
        RepoCatalog {
            watched: vec![RepoRef::new("acme", "web")],
            reachable: vec![
                RepoRef::new("hooli-inc", "hooli-api-legacy"),
                RepoRef::new("hooli-inc", "hooli-api"),
                RepoRef::new("acme", "web"),
                RepoRef::new("octo", "hooli-api-client"),
                RepoRef::new("octo", "tools"),
            ],
        }
    }

    fn groups(sections: &[(Group, Vec<PaletteItem>)]) -> Vec<Group> {
        sections.iter().map(|(group, _)| group.clone()).collect()
    }

    #[test]
    fn short_pull_request_reference_opens_the_pull_request() {
        assert_eq!(
            only_route("acme/web#123"),
            Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 123))
        );
    }

    #[test]
    fn pull_request_url_opens_the_pull_request() {
        assert_eq!(
            only_route("  https://github.com/acme/web/pull/7/files "),
            Route::PullRequest(PrRef::new(RepoRef::new("acme", "web"), 7))
        );
    }

    #[test]
    fn linear_identifier_is_uppercased() {
        assert_eq!(
            only_route("eng-123"),
            Route::LinearIssue {
                identifier: "ENG-123".into()
            }
        );
        assert_eq!(
            only_route("A1b2c3d4e5-9"),
            Route::LinearIssue {
                identifier: "A1B2C3D4E5-9".into()
            }
        );
    }

    #[test]
    fn bare_repository_opens_the_repository() {
        assert_eq!(
            only_route("acme/web"),
            Route::Repo(RepoRef::new("acme", "web"))
        );
        assert_eq!(
            only_route("https://github.com/acme/web.rs/"),
            Route::Repo(RepoRef::new("acme", "web.rs"))
        );
    }

    #[test]
    fn text_that_names_nothing_gives_no_item() {
        for query in [
            "",
            "hello",
            "acme/web#x",
            "acme/web#",
            "ENG-",
            "-12",
            "1ENG-12",
            "ENG-12a",
            "ABCDEFGHIJK-1",
            "ab-cd-12",
            "acme/web/extra",
            "https://linear.app/acme/issue/ENG-123/some-title",
            "https://example.com/acme/web",
            "pr:13",
            "repo:acme/web",
        ] {
            assert!(direct_items(query).is_empty(), "{query:?} should give none");
        }
    }

    #[test]
    fn typed_text_finds_the_split_commands_and_pages_do_not_list_them() {
        let labels = |query: &str| -> Vec<&str> {
            pane_commands(&ParsedQuery::parse(query))
                .into_iter()
                .map(|(label, _, _)| label)
                .collect()
        };
        assert_eq!(
            labels("split"),
            ["Split right", "Split left", "Split up", "Split down"]
        );
        assert_eq!(labels("Split Down"), ["Split down"]);
        assert!(labels("").is_empty(), "the empty palette lists pages");
        assert!(labels("pr: split").is_empty(), "a prefix narrows to pages");
        assert!(
            labels("spl rt").is_empty(),
            "a scattered match is no command"
        );
        for short in ["li", "do", "own", "right"] {
            assert!(
                labels(short).is_empty(),
                "{short:?} looks for a page, not a split"
            );
        }
    }

    #[test]
    fn rank_prefers_substring_then_subsequence() {
        let repo = item("acme/web", "Watched repository", Route::Inbox);
        assert_eq!(match_rank("web", &repo), Some(0));
        assert_eq!(match_rank("WATCHED", &repo), Some(0));
        assert_eq!(match_rank("amw", &repo), Some(1));
        assert_eq!(match_rank("zzz", &repo), None);
        assert_eq!(match_rank("  ", &repo), Some(0));
    }

    #[test]
    fn prefixes_split_from_the_text_in_any_order_and_case() {
        let parsed = ParsedQuery::parse("  repo:hooli-api  fix PR:login Org:acme issue:ux ");
        assert_eq!(
            parsed.prefixes,
            [
                Prefix::Repo,
                Prefix::PullRequest,
                Prefix::Org,
                Prefix::Issue
            ]
        );
        assert_eq!(parsed.repos, ["hooli-api"]);
        assert_eq!(parsed.orgs, ["acme"]);
        assert_eq!(parsed.text(), "fix login ux");

        let plural = ParsedQuery::parse("prs:x repos:y issues:z");
        assert_eq!(plural.repos, ["y"]);
        assert_eq!(plural.text(), "x z");
        assert!(plural.has(Prefix::PullRequest) && plural.has(Prefix::Issue));

        let bare = ParsedQuery::parse("pr:");
        assert_eq!(bare.prefixes, [Prefix::PullRequest]);
        assert!(bare.words.is_empty());

        let not_prefixes = ParsedQuery::parse("print: preview:x");
        assert!(not_prefixes.prefixes.is_empty());
        assert_eq!(not_prefixes.text(), "print: preview:x");
    }

    #[test]
    fn a_team_key_word_narrows_to_that_team() {
        let parsed = ParsedQuery::parse("back- proxy");
        assert_eq!(parsed.team_key.as_deref(), Some("BACK"));
        assert_eq!(parsed.text(), "proxy");
        assert_eq!(
            linear_search(&parsed),
            Some(("proxy".to_owned(), Some("BACK".to_owned())))
        );
        assert_eq!(
            linear_search(&ParsedQuery::parse("BACK-")),
            Some((String::new(), Some("BACK".to_owned())))
        );
        assert!(pr_searches(&parsed, &catalog()).is_empty());
        for word in ["ENG-290", "front-end", "-", "1A-"] {
            assert_eq!(ParsedQuery::parse(word).team_key, None, "{word:?}");
        }
    }

    #[test]
    fn prefix_ranges_mark_only_prefixes_that_start_a_word() {
        let text = "pr:13 repo:é-x  xpr:1 Issues: prs";
        assert_eq!(prefix_ranges(text), [0..3, 6..11, 23..30]);
        assert_eq!(&text[23..30], "Issues:");
        assert!(prefix_ranges("").is_empty());
    }

    #[test]
    fn pr_prefix_lists_only_pull_requests_over_the_known_accounts() {
        let parsed = ParsedQuery::parse("pr:13");
        let searches = pr_searches(&parsed, &catalog());
        assert_eq!(
            searches,
            [PrSearch {
                target: PrTarget::All,
                query: "is:pr 13 user:acme user:hooli-inc user:octo".into(),
                first: 10,
            }]
        );
        assert_eq!(linear_search(&parsed), None);

        let local = vec![item("Inbox", "GitHub", Route::Inbox)];
        let mut remote = RemoteResults::default();
        let generation = remote.reset();
        assert!(remote.expect(generation, &[Source::GitHub(PrTarget::All)]));
        assert!(remote.accept(
            &Source::GitHub(PrTarget::All),
            vec![pr_item("acme", "web", 13)],
            generation
        ));
        remote.linear = vec![item("ENG-13 Linear", "", Route::LinearMyIssues)];
        let sections = build_sections(&parsed, &[], &local, &catalog(), &remote);
        assert_eq!(groups(&sections), [Group::PullRequests]);
    }

    #[test]
    fn bare_pr_prefix_lists_the_newest_pull_requests() {
        let searches = pr_searches(&ParsedQuery::parse("pr:"), &RepoCatalog::default());
        assert_eq!(searches[0].query, "is:pr sort:updated-desc involves:@me");
    }

    #[test]
    fn repo_prefix_searches_each_named_repository() {
        let parsed = ParsedQuery::parse("repo:hooli-api pr:fix");
        let searches = pr_searches(&parsed, &catalog());
        let queries: Vec<&str> = searches.iter().map(|s| s.query.as_str()).collect();
        assert_eq!(
            queries,
            [
                "is:pr fix repo:hooli-inc/hooli-api",
                "is:pr fix repo:hooli-inc/hooli-api-legacy",
                "is:pr fix repo:octo/hooli-api-client",
            ],
            "the exact name first, then the others that hold it"
        );
        assert!(
            searches
                .iter()
                .all(|s| matches!(s.target, PrTarget::Repo(_)) && s.first == 8)
        );
        assert_eq!(linear_search(&parsed), None);

        let mut remote = RemoteResults::default();
        let generation = remote.reset();
        let sources: Vec<Source> = searches
            .iter()
            .map(|s| Source::GitHub(s.target.clone()))
            .collect();
        assert!(remote.expect(generation, &sources));
        assert!(remote.accept(
            &sources[1],
            vec![pr_item("hooli-inc", "hooli-api-legacy", 2)],
            generation
        ));
        assert!(remote.accept(
            &sources[0],
            vec![pr_item("hooli-inc", "hooli-api", 9)],
            generation
        ));
        let sections = build_sections(&parsed, &[], &[], &catalog(), &remote);
        let headings: Vec<String> = sections
            .iter()
            .map(|(group, _)| group.heading().to_string())
            .collect();
        assert_eq!(
            headings,
            [
                "Pull requests in hooli-inc/hooli-api",
                "Pull requests in hooli-inc/hooli-api-legacy",
            ],
            "pr: hides the repositories; sections keep the search order"
        );
    }

    #[test]
    fn repo_prefix_without_pr_lists_the_matching_repositories() {
        let parsed = ParsedQuery::parse("repo:hooli-api");
        let titles: Vec<String> = repo_rows(&parsed, &catalog())
            .iter()
            .map(|row| row.title.to_string())
            .collect();
        assert_eq!(
            titles,
            [
                "hooli-inc/hooli-api-legacy",
                "hooli-inc/hooli-api",
                "octo/hooli-api-client",
            ]
        );
        let full = pr_searches(&ParsedQuery::parse("repo:other/thing"), &catalog());
        assert_eq!(
            full[0].target,
            PrTarget::Repo(RepoRef::new("other", "thing"))
        );
        assert_eq!(full[0].query, "is:pr sort:updated-desc repo:other/thing");
    }

    #[test]
    fn org_prefix_narrows_repositories_and_the_search_to_that_account() {
        let parsed = ParsedQuery::parse("org:hooli login");
        assert_eq!(
            pr_searches(&parsed, &catalog())[0].query,
            "is:pr login user:hooli-inc"
        );
        let unknown = ParsedQuery::parse("org:rust-lang");
        assert_eq!(
            pr_searches(&unknown, &catalog())[0].query,
            "is:pr sort:updated-desc user:rust-lang"
        );
        let rows = repo_rows(&ParsedQuery::parse("org:octo"), &catalog());
        let titles: Vec<String> = rows.iter().map(|row| row.title.to_string()).collect();
        assert_eq!(titles, ["octo/hooli-api-client", "octo/tools"]);
    }

    #[test]
    fn issue_prefix_lists_only_linear_issues() {
        let parsed = ParsedQuery::parse("issue:proxy");
        assert!(pr_searches(&parsed, &catalog()).is_empty());
        assert_eq!(linear_search(&parsed), Some(("proxy".to_owned(), None)));
        let local = vec![item("Inbox", "proxy", Route::Inbox)];
        let mut remote = RemoteResults::default();
        remote.reset();
        remote.pull_requests = vec![pr_item("acme", "web", 1)];
        remote.linear = vec![item(
            "ENG-1 Proxy",
            "",
            Route::LinearIssue {
                identifier: "ENG-1".into(),
            },
        )];
        let sections = build_sections(&parsed, &[], &local, &catalog(), &remote);
        assert_eq!(groups(&sections), [Group::LinearIssues]);
    }

    #[test]
    fn plain_text_searches_everything() {
        let parsed = ParsedQuery::parse("web");
        assert_eq!(
            pr_searches(&parsed, &catalog())[0].query,
            "is:pr web user:acme user:hooli-inc user:octo"
        );
        assert_eq!(linear_search(&parsed), Some(("web".to_owned(), None)));
    }

    #[test]
    fn sections_group_filter_and_dedupe() {
        let pr = PrRef::new(RepoRef::new("acme", "web"), 1);
        let local = vec![
            item("Inbox", "GitHub", Route::Inbox),
            item("Settings", "Accounts", Route::Settings),
        ];
        let mut remote = RemoteResults::default();
        let generation = remote.reset();
        assert!(remote.expect(generation, &[Source::GitHub(PrTarget::All)]));
        assert!(remote.accept(
            &Source::GitHub(PrTarget::All),
            vec![item("Fix", "acme/web#1", Route::PullRequest(pr.clone()))],
            generation
        ));

        let all = build_sections(&ParsedQuery::default(), &[], &local, &catalog(), &remote);
        assert_eq!(
            groups(&all),
            [Group::GoTo, Group::Repositories, Group::PullRequests]
        );
        let (_, repos) = &all[1];
        assert_eq!(
            repos.len(),
            1,
            "an empty query lists the watched repositories"
        );

        let query = "acme/web#1";
        let filtered = build_sections(
            &ParsedQuery::parse(query),
            &direct_items(query),
            &local,
            &catalog(),
            &remote,
        );
        assert_eq!(
            groups(&filtered),
            [Group::JumpTo],
            "the direct jump wins over the remote duplicate, settings is filtered out"
        );
    }

    #[test]
    fn stale_remote_answer_is_dropped() {
        let mut remote = RemoteResults::default();
        let old = remote.reset();
        let new = remote.reset();
        assert!(!remote.expect(old, &[Source::Linear]));
        assert!(remote.expect(new, &[Source::Linear, Source::GitHub(PrTarget::All)]));
        assert!(remote.is_loading());

        let late = vec![item("Old", "", Route::Inbox)];
        assert!(!remote.accept(&Source::Linear, late, old));
        assert!(remote.linear.is_empty());
        assert!(remote.is_loading());

        let fresh = vec![item("New", "", Route::Notifications)];
        assert!(remote.accept(&Source::Linear, fresh, new));
        assert!(remote.accept(&Source::GitHub(PrTarget::All), Vec::new(), new));
        assert!(!remote.is_loading());
        assert_eq!(remote.linear.len(), 1);
    }

    #[test]
    fn typing_again_clears_previous_results() {
        let mut remote = RemoteResults::default();
        let first = remote.reset();
        let source = Source::GitHub(PrTarget::Repo(RepoRef::new("acme", "web")));
        assert!(remote.expect(first, std::slice::from_ref(&source)));
        remote.accept(&source, vec![item("Old", "", Route::Inbox)], first);
        assert_eq!(remote.repo_pull_requests.len(), 1);
        remote.reset();
        assert!(remote.repo_pull_requests.is_empty());
        assert!(!remote.is_loading());
    }

    #[test]
    fn the_status_names_what_is_searched() {
        let label = |query: &str| searching_label(&ParsedQuery::parse(query));
        assert_eq!(label("pr:"), "Searching pull requests…");
        assert_eq!(label("repo:web"), "Searching pull requests…");
        assert_eq!(label("issue:"), "Searching Linear issues…");
        assert_eq!(label("BACK- proxy"), "Searching Linear issues…");
        assert_eq!(label("login"), "Searching…");
    }

    mod item {
        use loupe_github::{PrRef, RepoRef};

        use super::super::{ParsedQuery, item_commands};
        use crate::linear::issue_menu::Field;
        use crate::linear::issue_menu::tests::issue;
        use crate::shell::item_menu::{Command, Item, Place, PrItem, sections};

        fn items() -> Vec<Item> {
            let mut with_branch = issue();
            with_branch.branch_name = "ana/eng-1-fix-it".into();
            vec![
                Item::issue(&issue()),
                Item::issue(&with_branch),
                Item::PullRequest(PrItem {
                    pr: PrRef::new(RepoRef::new("acme", "api"), 7),
                    title: "Fix the login".into(),
                    url: "https://github.com/acme/api/pull/7".into(),
                    head_ref: None,
                }),
                Item::GitHubIssue {
                    title: "Crash".into(),
                    url: "https://github.com/acme/api/issues/3".into(),
                },
            ]
        }

        /// Every command of a row's context menu is in the palette, in the
        /// menu's order, for the row under the pointer and for the page.
        #[test]
        fn the_palette_lists_every_command_of_the_items_menu() {
            for item in items() {
                for place in [Place::Row, Place::Page] {
                    for t3 in [false, true] {
                        let target = (item.clone(), place);
                        assert_eq!(
                            item_commands(Some(&target), t3, &ParsedQuery::parse("")),
                            sections(&item, place, t3).concat(),
                            "{} at {place:?}",
                            item.reference()
                        );
                    }
                }
            }
        }

        #[test]
        fn typed_text_narrows_the_items_commands() {
            let mut issue = issue();
            issue.branch_name = "ana/eng-1-fix-it".into();
            let target = (Item::issue(&issue), Place::Row);
            let find =
                |query: &str| item_commands(Some(&target), false, &ParsedQuery::parse(query));
            assert_eq!(
                find("copy"),
                [
                    Command::CopyLink,
                    Command::CopyId,
                    Command::CopyTitle,
                    Command::CopyBranch
                ]
            );
            assert_eq!(find("status"), [Command::Field(Field::Status)]);
            assert_eq!(find("linear"), [Command::OpenInBrowser]);
            assert!(find("s").is_empty(), "one letter is not a command yet");
            assert!(
                find("pr:copy").is_empty(),
                "a narrowed query lists pages only"
            );
            assert!(item_commands(None, false, &ParsedQuery::parse("")).is_empty());
        }
    }

    mod field {
        use gpui_kit::base::actions::Cancel;
        use gpui_kit::{
            AppContext as _, Context, Entity, IntoElement, Render, TestAppContext,
            VisualTestContext, Window, div, px,
        };

        use super::super::{PaletteSlot, PaletteView, Prefix, SEARCH_DEBOUNCE, toggle};
        use crate::services::Services;
        use crate::store::Settings;

        struct Blank;

        impl Render for Blank {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
            }
        }

        /// The palette open over an empty window, signed in nowhere, so no
        /// search leaves the machine.
        fn open_palette(cx: &mut TestAppContext) -> (VisualTestContext, Entity<PaletteView>) {
            let (handle, _) = cx.update(|cx| {
                gpui_kit::init(cx);
                crate::theme::init(cx);
                cx.set_global(Settings::default());
                cx.set_global(Services::default());
                gpui_kit::open_window(Default::default(), cx, |_, cx| cx.new(|_| Blank))
                    .expect("open a window")
            });
            let mut cx = VisualTestContext::from_window(handle, cx);
            cx.update(|window, cx| {
                toggle(None, window, cx);
                window.refresh();
                window.draw(cx).clear(cx);
            });
            cx.run_until_parked();
            let view = cx
                .update(|_, cx| cx.global::<PaletteSlot>().0.clone())
                .and_then(|view| view.upgrade())
                .expect("the palette is open");
            (cx, view)
        }

        #[gpui_kit::test]
        fn typed_prefixes_become_tokens_and_the_caret_stays_at_the_end(cx: &mut TestAppContext) {
            let (mut cx, view) = open_palette(cx);
            cx.simulate_input("pr:13 repo:web Issue:x");
            cx.run_until_parked();
            let (tokens, caret, narrowed) = cx.update(|_, cx| {
                let view = view.read(cx);
                let input = view.input.read(cx);
                let tokens: Vec<_> = input
                    .tokens()
                    .iter()
                    .map(|span| (span.range(), span.token().text().to_string()))
                    .collect();
                (
                    tokens,
                    input.selected_range(),
                    view.parsed.has(Prefix::Repo),
                )
            });
            assert_eq!(
                tokens,
                [
                    (0..3, "pr:".to_owned()),
                    (6..11, "repo:".to_owned()),
                    (15..21, "Issue:".to_owned()),
                ]
            );
            assert_eq!(caret, 22..22, "marking a prefix does not move the caret");
            assert!(narrowed, "the palette reads the query the field holds");
        }

        fn redraw(cx: &mut VisualTestContext) {
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }

        /// While a search with nothing to show yet runs, the list holds one
        /// status row as tall as a result row, not an empty strip.
        #[gpui_kit::test]
        fn a_search_with_no_rows_yet_shows_a_status_row(cx: &mut TestAppContext) {
            let (mut cx, view) = open_palette(cx);
            redraw(&mut cx);
            let first = cx.debug_bounds("palette-result-Notifications");
            let second = cx.debug_bounds("palette-result-Inbox");
            let (first, second) = first.zip(second).expect("the Go to rows");
            let row_pitch = second.origin.y - first.origin.y;
            assert!(row_pitch > px(0.));

            cx.simulate_input("pr:");
            redraw(&mut cx);
            assert!(cx.update(|_, cx| view.read(cx).is_searching()));
            let searching = cx
                .debug_bounds("palette-status")
                .expect("a status row while the search runs");
            assert_eq!(searching.size.height, row_pitch);

            cx.executor().advance_clock(SEARCH_DEBOUNCE * 2);
            redraw(&mut cx);
            assert!(
                !cx.update(|_, cx| view.read(cx).is_searching()),
                "the search ends, and the field stops spinning"
            );
            let settled = cx
                .debug_bounds("palette-status")
                .expect("a status row when nothing matched");
            // The dialog is still sliding in, so only the size compares.
            assert_eq!(settled.size, searching.size, "the palette keeps its layout");
        }

        #[gpui_kit::test]
        fn escape_clears_the_query_before_it_closes(cx: &mut TestAppContext) {
            let (mut cx, view) = open_palette(cx);
            cx.simulate_input("pr:13");
            cx.run_until_parked();
            cx.dispatch_action(Cancel);
            cx.run_until_parked();
            let (text, query) = cx.update(|_, cx| {
                let view = view.read(cx);
                (view.input.read(cx).value().to_string(), view.query.clone())
            });
            assert_eq!((text.as_str(), query.as_str()), ("", ""));
            assert!(
                cx.update(|window, cx| {
                    use gpui_kit::component::WindowExt as _;
                    window.has_active_dialog(cx)
                }),
                "the first Escape leaves the palette open"
            );
        }

        /// S on a row opens its status list: typing narrows it, Enter picks,
        /// and the issue changes at once.
        #[gpui_kit::test]
        fn a_property_list_narrows_and_enter_picks(cx: &mut TestAppContext) {
            use gpui_kit::base::actions::Confirm;

            use crate::linear::issue_menu::Field;
            use crate::linear::issue_menu::tests::{issue, refusing_linear, team};

            let (mut cx, published) = refusing_linear(cx);
            cx.update(|window, cx| {
                crate::linear::issue_menu::tests::cache_team(team(), cx);
                super::super::open_picker(issue(), Field::Status, window, cx);
            });
            redraw(&mut cx);
            assert!(cx.debug_bounds("palette-choice-state-todo").is_some());
            assert!(cx.debug_bounds("palette-choice-state-done").is_some());

            cx.simulate_input("don");
            redraw(&mut cx);
            assert!(
                cx.debug_bounds("palette-choice-state-todo").is_none(),
                "typing narrows the list"
            );
            cx.dispatch_action(Confirm { secondary: false });
            assert_eq!(published.borrow().first().map(String::as_str), Some("done"));
            assert!(
                !cx.update(|window, cx| {
                    use gpui_kit::component::WindowExt as _;
                    window.has_active_dialog(cx)
                }),
                "picking closes the palette"
            );
        }
    }
}
