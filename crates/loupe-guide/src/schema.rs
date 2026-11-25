//! Versioned schema, strict boundary validation, and cross-commit step diffing
//! for guided pull request reviews.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Component, Path};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::language::GuideLanguage;

/// The only guide schema version supported by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// Maximum number of lines permitted in a single diff snippet.
pub const MAX_SNIPPET_LINES: usize = 80;

/// Maximum byte length permitted in a single diff snippet.
pub const MAX_SNIPPET_BYTES: usize = 8 * 1024;

/// A validated guided review for a pull request at one commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewGuide {
    pub schema_version: u32,
    pub summary: GuideSummary,
    pub decisions: Vec<DecisionPoint>,
    pub steps: Vec<GuideStep>,
    pub removed_tests: Vec<RemovedTest>,
}

/// High-level story of the change shown above the step-by-step walkthrough.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideSummary {
    /// One-sentence summary of what changed and why.
    pub headline: String,
    /// Concise Markdown explaining the motivation and direction of the change.
    pub why: String,
    /// Optional before/after comparison when execution or data flow changes.
    pub flow: Option<FlowComparison>,
    /// Callers, screens, or subsystems that use this change.
    pub callers: Vec<String>,
    /// Contracts or invariants between parts of the system.
    pub contracts: Vec<String>,
}

/// How execution or data flow changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowComparison {
    pub before: String,
    pub after: String,
}

/// Classification of a behavior change that requires human reviewer sign-off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionCategory {
    Behavior,
    Data,
    Money,
    EventFormat,
    Security,
    Api,
    Other,
}

impl DecisionCategory {
    pub fn label(self) -> &'static str {
        match self {
            DecisionCategory::Behavior => "Behavior",
            DecisionCategory::Data => "Data",
            DecisionCategory::Money => "Money",
            DecisionCategory::EventFormat => "Event format",
            DecisionCategory::Security => "Security",
            DecisionCategory::Api => "API",
            DecisionCategory::Other => "Other",
        }
    }
}

/// A behavior change that a human reviewer must consciously accept.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionPoint {
    pub id: String,
    pub title: String,
    pub category: DecisionCategory,
    pub impact: String,
    /// ID of the [`GuideStep`] that explains this decision.
    pub step_id: String,
    /// Optional index into that step's [`GuideStep::snippets`].
    pub snippet_index: Option<usize>,
}

/// Role of a step in the narrative walkthrough.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepKind {
    Overview,
    Flow,
    Callers,
    Contract,
    Subsystem,
    Tests,
}

impl StepKind {
    pub fn label(self) -> &'static str {
        match self {
            StepKind::Overview => "Overview",
            StepKind::Flow => "Flow",
            StepKind::Callers => "Callers",
            StepKind::Contract => "Contract",
            StepKind::Subsystem => "Subsystem",
            StepKind::Tests => "Tests",
        }
    }
}

/// One step in the ordered walkthrough of the pull request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuideStep {
    pub id: String,
    pub kind: StepKind,
    pub title: String,
    /// Markdown explaining this part of the change.
    pub explanation: String,
    /// Short diff excerpts that matter for this step.
    pub snippets: Vec<DiffSnippet>,
}

/// Which side of the diff a snippet line range anchors to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnippetSide {
    #[default]
    New,
    Old,
}

/// A focused diff excerpt inside a step, clickable to open the Files tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffSnippet {
    /// Repository-relative path (POSIX separators, no leading `/` or `..`).
    pub path: String,
    /// 1-based start line on `side`.
    pub start_line: u32,
    /// 1-based inclusive end line on `side` (`>= start_line`).
    pub end_line: u32,
    pub side: SnippetSide,
    /// One-line note explaining why this excerpt matters.
    pub caption: String,
    /// Short unified-diff lines (`+`, `-`, ` `) showing only what is new or changed.
    pub diff: String,
}

/// A test removed by the pull request with no evident replacement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemovedTest {
    pub name: String,
    pub path: String,
    /// Optional 1-based line number on the base (`Old`) side of the diff.
    pub line: Option<u32>,
    pub reason: String,
}

/// A cached guide together with authoritative provenance stamped by Loupe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredGuide {
    pub schema_version: u32,
    pub repo: String,
    pub pr_number: u64,
    pub head_oid: String,
    pub base_oid: String,
    pub model: String,
    pub effort: String,
    pub generated_at: DateTime<Utc>,
    /// Authoritative list of files changed in the pull request (from GitHub, not the model),
    /// used to show files the guide did not cover with any snippet.
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// Language the guide's prose was written in. Guides saved before Loupe
    /// asked for a language were written in English.
    #[serde(default = "GuideLanguage::english")]
    pub language: GuideLanguage,
    pub guide: ReviewGuide,
}

/// Returns repository-relative paths in `changed_files` that are not referenced
/// by any diff snippet or removed-test entry in `guide`.
pub fn uncovered_files(changed_files: &[String], guide: &ReviewGuide) -> Vec<String> {
    let mut covered: HashSet<&str> = HashSet::new();
    for step in &guide.steps {
        for snippet in &step.snippets {
            covered.insert(snippet.path.as_str());
        }
    }
    for removed in &guide.removed_tests {
        covered.insert(removed.path.as_str());
    }
    changed_files
        .iter()
        .filter(|path| !covered.contains(path.as_str()))
        .cloned()
        .collect()
}

/// Validates that an identifier is a short lowercase kebab-case slug
/// (`^[a-z0-9]+(-[a-z0-9]+)*$`, 1..=64 ASCII characters) so cached IDs can
/// never smuggle newlines or prompt-injection text.
pub fn is_valid_slug(id: &str) -> bool {
    if id.is_empty() || id.len() > 64 {
        return false;
    }
    let bytes = id.as_bytes();
    if bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    let mut prev_dash = false;
    for &b in bytes {
        let ok = b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-';
        if !ok || (b == b'-' && prev_dash) {
            return false;
        }
        prev_dash = b == b'-';
    }
    true
}

impl StoredGuide {
    /// Whether this cached guide matches `current_head_oid`.
    pub fn is_stale(&self, current_head_oid: &str) -> bool {
        self.head_oid != current_head_oid
    }

    /// Short 7-character commit prefix for display.
    pub fn short_sha(&self) -> &str {
        self.head_oid.get(..7).unwrap_or(&self.head_oid)
    }
}

/// Per-commit viewed marks for a pull request's guide.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuideReadState {
    pub viewed_steps: BTreeSet<String>,
}

impl GuideReadState {
    pub fn is_viewed(&self, step_id: &str) -> bool {
        self.viewed_steps.contains(step_id)
    }

    pub fn toggle(&mut self, step_id: &str) -> bool {
        if self.viewed_steps.remove(step_id) {
            false
        } else {
            self.viewed_steps.insert(step_id.to_owned());
            true
        }
    }

    pub fn viewed_count(&self, guide: &ReviewGuide) -> usize {
        guide
            .steps
            .iter()
            .filter(|step| self.viewed_steps.contains(&step.id))
            .count()
    }
}

/// How a step changed relative to the last guide the reviewer read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepChange {
    New,
    Updated,
}

impl StepChange {
    pub fn label(self) -> &'static str {
        match self {
            StepChange::New => "New",
            StepChange::Updated => "Updated",
        }
    }
}

/// Why a guide JSON payload was rejected.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GuideValidationError {
    #[error("Invalid guide JSON: {0}")]
    MalformedJson(String),
    #[error("Unsupported guide schema version {found} (supported: {supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("Missing required text in `{field}`")]
    EmptyField { field: String },
    #[error("Invalid identifier `{id}` in `{field}`: must be lowercase kebab-case (1-64 chars)")]
    InvalidSlug { field: String, id: String },
    #[error("Guide must contain at least one walkthrough step")]
    NoSteps,
    #[error("Duplicate step id `{id}`")]
    DuplicateStepId { id: String },
    #[error("Duplicate decision id `{id}`")]
    DuplicateDecisionId { id: String },
    #[error("Decision `{decision_id}` references unknown step `{step_id}`")]
    UnknownDecisionStep {
        decision_id: String,
        step_id: String,
    },
    #[error(
        "Decision `{decision_id}` references snippet index {index}, but step `{step_id}` has {count} snippet(s)"
    )]
    InvalidDecisionSnippet {
        decision_id: String,
        step_id: String,
        index: usize,
        count: usize,
    },
    #[error("Invalid path `{path}` in `{field}`: paths must be relative without `..`")]
    InvalidPath { field: String, path: String },
    #[error("Invalid line range {start}..={end} in `{field}`")]
    InvalidLineRange { field: String, start: u32, end: u32 },
    #[error("Invalid line number 0 in `{field}`")]
    ZeroLine { field: String },
    #[error("Snippet `{field}` exceeds maximum size ({lines} lines, {bytes} bytes)")]
    SnippetTooLarge {
        field: String,
        lines: usize,
        bytes: usize,
    },
}

/// Parses a JSON string as a [`ReviewGuide`] and validates all structural and
/// cross-reference invariants.
pub fn parse_and_validate(json: &str) -> Result<ReviewGuide, GuideValidationError> {
    let guide: ReviewGuide = serde_json::from_str(json)
        .map_err(|err| GuideValidationError::MalformedJson(err.to_string()))?;
    validate(&guide)?;
    Ok(guide)
}

/// Validates a parsed [`ReviewGuide`] against all schema v1 invariants.
pub fn validate(guide: &ReviewGuide) -> Result<(), GuideValidationError> {
    if guide.schema_version != SCHEMA_VERSION {
        return Err(GuideValidationError::UnsupportedVersion {
            found: guide.schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    require_non_empty(&guide.summary.headline, "summary.headline")?;
    require_non_empty(&guide.summary.why, "summary.why")?;
    if let Some(flow) = &guide.summary.flow {
        require_non_empty(&flow.before, "summary.flow.before")?;
        require_non_empty(&flow.after, "summary.flow.after")?;
    }
    for (ix, caller) in guide.summary.callers.iter().enumerate() {
        require_non_empty(caller, &format!("summary.callers[{ix}]"))?;
    }
    for (ix, contract) in guide.summary.contracts.iter().enumerate() {
        require_non_empty(contract, &format!("summary.contracts[{ix}]"))?;
    }
    if guide.steps.is_empty() {
        return Err(GuideValidationError::NoSteps);
    }

    let mut step_snippets: HashMap<&str, usize> = HashMap::with_capacity(guide.steps.len());
    for (ix, step) in guide.steps.iter().enumerate() {
        require_slug(&step.id, &format!("steps[{ix}].id"))?;
        require_non_empty(&step.title, &format!("steps[{ix}].title"))?;
        require_non_empty(&step.explanation, &format!("steps[{ix}].explanation"))?;
        if step_snippets
            .insert(step.id.as_str(), step.snippets.len())
            .is_some()
        {
            return Err(GuideValidationError::DuplicateStepId {
                id: step.id.clone(),
            });
        }
        for (snip_ix, snippet) in step.snippets.iter().enumerate() {
            let prefix = format!("steps[{ix}].snippets[{snip_ix}]");
            validate_relative_path(&snippet.path, &format!("{prefix}.path"))?;
            if snippet.start_line == 0 || snippet.end_line < snippet.start_line {
                return Err(GuideValidationError::InvalidLineRange {
                    field: format!("{prefix}.start_line"),
                    start: snippet.start_line,
                    end: snippet.end_line,
                });
            }
            require_non_empty(&snippet.caption, &format!("{prefix}.caption"))?;
            require_non_empty(&snippet.diff, &format!("{prefix}.diff"))?;
            let line_count = snippet.diff.lines().count();
            let byte_len = snippet.diff.len();
            if line_count > MAX_SNIPPET_LINES || byte_len > MAX_SNIPPET_BYTES {
                return Err(GuideValidationError::SnippetTooLarge {
                    field: format!("{prefix}.diff"),
                    lines: line_count,
                    bytes: byte_len,
                });
            }
        }
    }

    let mut decision_ids: HashSet<&str> = HashSet::with_capacity(guide.decisions.len());
    for (ix, decision) in guide.decisions.iter().enumerate() {
        require_slug(&decision.id, &format!("decisions[{ix}].id"))?;
        require_non_empty(&decision.title, &format!("decisions[{ix}].title"))?;
        require_non_empty(&decision.impact, &format!("decisions[{ix}].impact"))?;
        if !decision_ids.insert(decision.id.as_str()) {
            return Err(GuideValidationError::DuplicateDecisionId {
                id: decision.id.clone(),
            });
        }
        let Some(&snippet_count) = step_snippets.get(decision.step_id.as_str()) else {
            return Err(GuideValidationError::UnknownDecisionStep {
                decision_id: decision.id.clone(),
                step_id: decision.step_id.clone(),
            });
        };
        if let Some(snippet_index) = decision.snippet_index
            && snippet_index >= snippet_count
        {
            return Err(GuideValidationError::InvalidDecisionSnippet {
                decision_id: decision.id.clone(),
                step_id: decision.step_id.clone(),
                index: snippet_index,
                count: snippet_count,
            });
        }
    }

    for (ix, removed) in guide.removed_tests.iter().enumerate() {
        let prefix = format!("removed_tests[{ix}]");
        require_non_empty(&removed.name, &format!("{prefix}.name"))?;
        validate_relative_path(&removed.path, &format!("{prefix}.path"))?;
        if removed.line == Some(0) {
            return Err(GuideValidationError::ZeroLine {
                field: format!("{prefix}.line"),
            });
        }
        require_non_empty(&removed.reason, &format!("{prefix}.reason"))?;
    }

    Ok(())
}

fn require_non_empty(value: &str, field: &str) -> Result<(), GuideValidationError> {
    if value.trim().is_empty() {
        Err(GuideValidationError::EmptyField {
            field: field.to_owned(),
        })
    } else {
        Ok(())
    }
}

fn require_slug(value: &str, field: &str) -> Result<(), GuideValidationError> {
    if is_valid_slug(value) {
        Ok(())
    } else {
        Err(GuideValidationError::InvalidSlug {
            field: field.to_owned(),
            id: value.to_owned(),
        })
    }
}

fn validate_relative_path(path: &str, field: &str) -> Result<(), GuideValidationError> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed != path
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.contains('\0')
        || trimmed.contains('\\')
    {
        return Err(GuideValidationError::InvalidPath {
            field: field.to_owned(),
            path: path.to_owned(),
        });
    }
    let parsed = Path::new(trimmed);
    let mut has_normal = false;
    for component in parsed.components() {
        match component {
            Component::Normal(_) => has_normal = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(GuideValidationError::InvalidPath {
                    field: field.to_owned(),
                    path: path.to_owned(),
                });
            }
        }
    }
    if !has_normal {
        return Err(GuideValidationError::InvalidPath {
            field: field.to_owned(),
            path: path.to_owned(),
        });
    }
    Ok(())
}

/// Identifies steps in `current` that are new or changed compared to `previous`.
///
/// Steps match primarily by `id`. When a step ID exists in `previous`, it is
/// marked [`StepChange::Updated`] if its kind, title, explanation, snippets, or
/// linked decision points differ. Steps whose ID is absent from `previous` are
/// marked [`StepChange::New`].
pub fn changed_steps_since(
    previous: &ReviewGuide,
    current: &ReviewGuide,
) -> HashMap<String, StepChange> {
    let prev_steps: HashMap<&str, &GuideStep> = previous
        .steps
        .iter()
        .map(|step| (step.id.as_str(), step))
        .collect();
    let prev_decisions = decisions_by_step(previous);
    let curr_decisions = decisions_by_step(current);

    let mut changed = HashMap::new();
    for step in &current.steps {
        match prev_steps.get(step.id.as_str()) {
            None => {
                changed.insert(step.id.clone(), StepChange::New);
            }
            Some(old_step) => {
                let old_decs = prev_decisions.get(step.id.as_str());
                let new_decs = curr_decisions.get(step.id.as_str());
                if *old_step != step || old_decs != new_decs {
                    changed.insert(step.id.clone(), StepChange::Updated);
                }
            }
        }
    }
    changed
}

fn decisions_by_step(guide: &ReviewGuide) -> HashMap<&str, Vec<&DecisionPoint>> {
    let mut grouped: HashMap<&str, Vec<&DecisionPoint>> = HashMap::new();
    for decision in &guide.decisions {
        grouped
            .entry(decision.step_id.as_str())
            .or_default()
            .push(decision);
    }
    grouped
}

/// Returns the JSON Schema passed to Claude Code's `--json-schema` flag.
///
/// Every object sets `additionalProperties: false` and lists all properties as
/// required (nullable where optional in Rust) to satisfy strict structured
/// output decoding.
pub fn guide_json_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "summary", "decisions", "steps", "removed_tests"],
        "properties": {
            "schema_version": {
                "type": "integer",
                "enum": [SCHEMA_VERSION]
            },
            "summary": {
                "type": "object",
                "additionalProperties": false,
                "required": ["headline", "why", "flow", "callers", "contracts"],
                "properties": {
                    "headline": { "type": "string" },
                    "why": { "type": "string" },
                    "flow": {
                        "anyOf": [
                            { "type": "null" },
                            {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["before", "after"],
                                "properties": {
                                    "before": { "type": "string" },
                                    "after": { "type": "string" }
                                }
                            }
                        ]
                    },
                    "callers": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "contracts": {
                        "type": "array",
                        "items": { "type": "string" }
                    }
                }
            },
            "decisions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "title", "category", "impact", "step_id", "snippet_index"],
                    "properties": {
                        "id": { "type": "string", "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$", "maxLength": 64 },
                        "title": { "type": "string" },
                        "category": {
                            "type": "string",
                            "enum": ["behavior", "data", "money", "event_format", "security", "api", "other"]
                        },
                        "impact": { "type": "string" },
                        "step_id": { "type": "string" },
                        "snippet_index": {
                            "anyOf": [
                                { "type": "null" },
                                { "type": "integer", "minimum": 0 }
                            ]
                        }
                    }
                }
            },
            "steps": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "kind", "title", "explanation", "snippets"],
                    "properties": {
                        "id": { "type": "string", "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$", "maxLength": 64 },
                        "kind": {
                            "type": "string",
                            "enum": ["overview", "flow", "callers", "contract", "subsystem", "tests"]
                        },
                        "title": { "type": "string" },
                        "explanation": { "type": "string" },
                        "snippets": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["path", "start_line", "end_line", "side", "caption", "diff"],
                                "properties": {
                                    "path": { "type": "string" },
                                    "start_line": { "type": "integer", "minimum": 1 },
                                    "end_line": { "type": "integer", "minimum": 1 },
                                    "side": {
                                        "type": "string",
                                        "enum": ["new", "old"]
                                    },
                                    "caption": { "type": "string" },
                                    "diff": { "type": "string" }
                                }
                            }
                        }
                    }
                }
            },
            "removed_tests": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["name", "path", "line", "reason"],
                    "properties": {
                        "name": { "type": "string" },
                        "path": { "type": "string" },
                        "line": {
                            "anyOf": [
                                { "type": "null" },
                                { "type": "integer", "minimum": 1 }
                            ]
                        },
                        "reason": { "type": "string" }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::{
        DecisionCategory, DecisionPoint, DiffSnippet, FlowComparison, GuideReadState, GuideStep,
        GuideSummary, GuideValidationError, MAX_SNIPPET_LINES, RemovedTest, ReviewGuide,
        SCHEMA_VERSION, SnippetSide, StepChange, StepKind, StoredGuide, changed_steps_since,
        guide_json_schema, parse_and_validate, validate,
    };

    fn sample_guide() -> ReviewGuide {
        ReviewGuide {
            schema_version: SCHEMA_VERSION,
            summary: GuideSummary {
                headline: "Move token validation into a dedicated boundary parser".into(),
                why: "Callers previously re-parsed headers in three places, causing subtle mismatches.".into(),
                flow: Some(FlowComparison {
                    before: "HTTP handler -> ad-hoc header split -> service".into(),
                    after: "HTTP handler -> AuthHeader::parse -> typed Token -> service".into(),
                }),
                callers: vec!["crates/api/src/routes.rs".into()],
                contracts: vec!["Service functions only accept validated Token values".into()],
            },
            decisions: vec![DecisionPoint {
                id: "reject-bare-tokens".into(),
                title: "Reject authorization headers without Bearer prefix".into(),
                category: DecisionCategory::Api,
                impact: "Clients sending a raw token without `Bearer ` now receive 401.".into(),
                step_id: "parser-boundary".into(),
                snippet_index: Some(0),
            }],
            steps: vec![
                GuideStep {
                    id: "parser-boundary".into(),
                    kind: StepKind::Contract,
                    title: "Parse and brand tokens at the HTTP boundary".into(),
                    explanation: "Introduces `AuthHeader::parse` so downstream handlers receive a validated token.".into(),
                    snippets: vec![DiffSnippet {
                        path: "crates/api/src/auth.rs".into(),
                        start_line: 14,
                        end_line: 22,
                        side: SnippetSide::New,
                        caption: "Enforce Bearer prefix at the boundary".into(),
                        diff: "+pub fn parse(raw: &str) -> Result<Token, AuthError> {\n+    let rest = raw.strip_prefix(\"Bearer \").ok_or(AuthError::MissingScheme)?;\n+    Ok(Token(rest.to_owned()))\n+}".into(),
                    }],
                },
                GuideStep {
                    id: "unit-tests".into(),
                    kind: StepKind::Tests,
                    title: "Cover valid and missing-scheme headers".into(),
                    explanation: "Verified the test body asserts `Err(AuthError::MissingScheme)` on bare headers.".into(),
                    snippets: Vec::new(),
                },
            ],
            removed_tests: vec![RemovedTest {
                name: "accepts_legacy_raw_token_header".into(),
                path: "crates/api/src/auth.rs".into(),
                line: Some(88),
                reason: "Removed because bare tokens are no longer accepted; confirm no legacy client relies on it.".into(),
            }],
        }
    }

    #[test]
    fn valid_guide_roundtrips_and_validates() {
        let guide = sample_guide();
        let json = serde_json::to_string_pretty(&guide).expect("serialize");
        let parsed = parse_and_validate(&json).expect("validate");
        assert_eq!(parsed, guide);
    }

    #[test]
    fn rejects_malformed_json_unknown_fields_and_wrong_version() {
        assert!(matches!(
            parse_and_validate("{not json"),
            Err(GuideValidationError::MalformedJson(_))
        ));

        let mut raw = serde_json::to_value(sample_guide()).unwrap();
        raw.as_object_mut()
            .unwrap()
            .insert("unexpected".into(), serde_json::json!(true));
        let err = parse_and_validate(&raw.to_string()).unwrap_err();
        assert!(matches!(err, GuideValidationError::MalformedJson(_)));

        let mut wrong_version = sample_guide();
        wrong_version.schema_version = 99;
        assert_eq!(
            validate(&wrong_version),
            Err(GuideValidationError::UnsupportedVersion {
                found: 99,
                supported: SCHEMA_VERSION,
            })
        );
    }

    #[test]
    fn rejects_empty_required_fields_and_empty_steps() {
        let mut guide = sample_guide();
        guide.summary.headline = "   ".into();
        assert!(matches!(
            validate(&guide),
            Err(GuideValidationError::EmptyField { field }) if field == "summary.headline"
        ));

        let mut no_steps = sample_guide();
        no_steps.steps.clear();
        no_steps.decisions.clear();
        assert_eq!(validate(&no_steps), Err(GuideValidationError::NoSteps));

        // Slugs with newlines, spaces, or uppercase are rejected to block prompt injection.
        for bad_slug in [
            "",
            "a\nSYSTEM OVERRIDE",
            "UpperCase",
            "-leading",
            "trailing-",
            "double--dash",
            "has space",
        ] {
            let mut bad = sample_guide();
            bad.steps[0].id = bad_slug.into();
            bad.decisions[0].step_id = bad_slug.into();
            assert!(
                matches!(
                    validate(&bad),
                    Err(GuideValidationError::InvalidSlug { .. })
                ),
                "slug `{bad_slug}` must be rejected"
            );
        }
    }

    #[test]
    fn rejects_duplicate_ids_and_broken_decision_references() {
        let mut dup_step = sample_guide();
        dup_step.steps[1].id = dup_step.steps[0].id.clone();
        assert_eq!(
            validate(&dup_step),
            Err(GuideValidationError::DuplicateStepId {
                id: "parser-boundary".into()
            })
        );

        let mut bad_step_ref = sample_guide();
        bad_step_ref.decisions[0].step_id = "missing-step".into();
        assert_eq!(
            validate(&bad_step_ref),
            Err(GuideValidationError::UnknownDecisionStep {
                decision_id: "reject-bare-tokens".into(),
                step_id: "missing-step".into(),
            })
        );

        let mut bad_snippet = sample_guide();
        bad_snippet.decisions[0].snippet_index = Some(3);
        assert_eq!(
            validate(&bad_snippet),
            Err(GuideValidationError::InvalidDecisionSnippet {
                decision_id: "reject-bare-tokens".into(),
                step_id: "parser-boundary".into(),
                index: 3,
                count: 1,
            })
        );
    }

    #[test]
    fn rejects_unsafe_paths_bad_line_ranges_and_oversized_snippets() {
        for bad_path in [
            "",
            "/etc/passwd",
            "../secret.rs",
            "a/../../b.rs",
            "./a.rs",
            "a\\b.rs",
        ] {
            let mut guide = sample_guide();
            guide.steps[0].snippets[0].path = bad_path.into();
            assert!(
                matches!(
                    validate(&guide),
                    Err(GuideValidationError::InvalidPath { .. })
                ),
                "path `{bad_path}` must be rejected"
            );
        }

        let mut bad_lines = sample_guide();
        bad_lines.steps[0].snippets[0].start_line = 0;
        assert!(matches!(
            validate(&bad_lines),
            Err(GuideValidationError::InvalidLineRange { .. })
        ));

        let mut reversed = sample_guide();
        reversed.steps[0].snippets[0].start_line = 20;
        reversed.steps[0].snippets[0].end_line = 10;
        assert!(matches!(
            validate(&reversed),
            Err(GuideValidationError::InvalidLineRange { .. })
        ));

        let mut huge = sample_guide();
        huge.steps[0].snippets[0].diff = (0..=MAX_SNIPPET_LINES)
            .map(|i| format!("+line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(matches!(
            validate(&huge),
            Err(GuideValidationError::SnippetTooLarge { .. })
        ));

        let mut zero_removed = sample_guide();
        zero_removed.removed_tests[0].line = Some(0);
        assert_eq!(
            validate(&zero_removed),
            Err(GuideValidationError::ZeroLine {
                field: "removed_tests[0].line".into()
            })
        );
    }

    #[test]
    fn tracks_viewed_steps_staleness_and_changes_across_commits() {
        let guide_v1 = sample_guide();
        let stored = StoredGuide {
            schema_version: SCHEMA_VERSION,
            repo: "IsraelAraujo70/loupe".into(),
            pr_number: 140,
            head_oid: "aaaa1111bbbb2222".into(),
            base_oid: "0000111122223333".into(),
            model: "claude-sonnet-5-5".into(),
            effort: "medium".into(),
            generated_at: Utc::now(),
            changed_files: vec![
                "crates/api/src/auth.rs".into(),
                "crates/api/src/telemetry.rs".into(),
            ],
            language: crate::GuideLanguage::english(),
            guide: guide_v1.clone(),
        };
        assert_eq!(
            super::uncovered_files(&stored.changed_files, &stored.guide),
            vec!["crates/api/src/telemetry.rs".to_owned()]
        );
        assert!(!stored.is_stale("aaaa1111bbbb2222"));
        assert!(stored.is_stale("cccc3333dddd4444"));
        assert_eq!(stored.short_sha(), "aaaa111");

        let mut read_state = GuideReadState::default();
        assert_eq!(read_state.viewed_count(&guide_v1), 0);
        assert!(read_state.toggle("parser-boundary"));
        assert!(read_state.is_viewed("parser-boundary"));
        assert_eq!(read_state.viewed_count(&guide_v1), 1);
        assert!(!read_state.toggle("parser-boundary"));
        assert_eq!(read_state.viewed_count(&guide_v1), 0);

        let mut guide_v2 = guide_v1.clone();
        guide_v2.steps[0].explanation = "Updated explanation after review feedback.".into();
        guide_v2.steps.push(GuideStep {
            id: "error-mapping".into(),
            kind: StepKind::Subsystem,
            title: "Map AuthError to HTTP 401".into(),
            explanation: "Returns a structured JSON error body.".into(),
            snippets: Vec::new(),
        });

        let diff = changed_steps_since(&guide_v1, &guide_v2);
        assert_eq!(diff.get("parser-boundary"), Some(&StepChange::Updated));
        assert_eq!(diff.get("unit-tests"), None);
        assert_eq!(diff.get("error-mapping"), Some(&StepChange::New));
    }

    #[test]
    fn json_schema_matches_top_level_contract() {
        let schema = guide_json_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(
            schema["properties"]["schema_version"]["enum"][0],
            SCHEMA_VERSION
        );
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            required,
            [
                "schema_version",
                "summary",
                "decisions",
                "steps",
                "removed_tests"
            ]
        );
    }

    #[test]
    fn stored_guide_records_its_language_and_loads_guides_saved_without_one() {
        let mut stored = serde_json::json!({
            "schema_version": SCHEMA_VERSION,
            "repo": "IsraelAraujo70/loupe",
            "pr_number": 140,
            "head_oid": "aaaa1111bbbb2222",
            "base_oid": "0000111122223333",
            "model": "claude-sonnet-5-5",
            "effort": "medium",
            "generated_at": "2026-11-20T12:00:00Z",
            "changed_files": [],
            "guide": sample_guide(),
        });
        let legacy: StoredGuide =
            serde_json::from_value(stored.clone()).expect("a guide saved before #143 loads");
        assert_eq!(legacy.language, crate::GuideLanguage::english());

        stored["language"] = "Português (Brasil)".into();
        let localized: StoredGuide = serde_json::from_value(stored).expect("loads");
        assert_eq!(
            localized.language,
            crate::GuideLanguage::portuguese_brazil()
        );
        let round_trip: StoredGuide =
            serde_json::from_str(&serde_json::to_string(&localized).unwrap()).unwrap();
        assert_eq!(round_trip.language, localized.language);
    }
}
