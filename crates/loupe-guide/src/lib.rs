//! Native guided pull request review schema, validation, and local generation.

pub mod checkout;
pub mod claude;
pub mod prompt;
pub mod runner;
pub mod schema;

pub use checkout::{CheckoutError, TempCheckout, snapshot_tree};
pub use claude::{
    ClaudeAvailability, ClaudeConfig, DISALLOWED_TOOLS, GuideEffort, GuideModel,
    PERMITTED_INIT_TOOLS, READ_ONLY_TOOLS, StreamEvent, build_command, check_availability,
    parse_stream_line, resolve_executable, unexpected_init_tools,
};
pub use prompt::{
    GuidePromptInput, PromptComment, PromptCommit, PromptFile, SYSTEM_PROMPT, build_user_prompt,
};
pub use runner::{
    CancelToken, DEFAULT_AGENT_TIMEOUT, GeneratedGuide, GuideRunError, ProgressEvent,
    generate_for_pr, run_in_checkout, run_in_checkout_with_timeout,
};
pub use schema::{
    DecisionCategory, DecisionPoint, DiffSnippet, FlowComparison, GuideReadState, GuideStep,
    GuideSummary, GuideValidationError, RemovedTest, ReviewGuide, SCHEMA_VERSION, SnippetSide,
    StepChange, StepKind, StoredGuide, changed_steps_since, guide_json_schema, is_valid_slug,
    parse_and_validate, uncovered_files, validate,
};
