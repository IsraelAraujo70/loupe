//! Offline gate: every GraphQL document the crate sends must validate against
//! the checked-in GitHub schema.

use apollo_compiler::{ExecutableDocument, Schema};

#[test]
fn every_document_validates_against_the_schema() {
    let sdl = include_str!("../../../schemas/github.graphql");
    let schema = Schema::parse_and_validate(sdl, "github.graphql")
        .expect("the checked-in GitHub schema is valid");
    let documents = loupe_github::graphql_documents();
    assert!(!documents.is_empty());
    let mut failures = Vec::new();
    for (name, source) in documents {
        if let Err(errors) = ExecutableDocument::parse_and_validate(&schema, source, name) {
            failures.push(format!("{name}:\n{}", errors.errors));
        }
    }
    assert!(
        failures.is_empty(),
        "invalid documents:\n{}",
        failures.join("\n")
    );
}
