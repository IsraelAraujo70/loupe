//! Native guided pull request review schema, validation, and local generation.

pub mod schema;

pub use schema::{
    DecisionCategory, DecisionPoint, DiffSnippet, FlowComparison, GuideReadState, GuideStep,
    GuideSummary, GuideValidationError, RemovedTest, ReviewGuide, SCHEMA_VERSION, SnippetSide,
    StepChange, StepKind, StoredGuide, changed_steps_since, guide_json_schema, parse_and_validate,
    validate,
};
