//! Offline schema gate: every document in `documents::ALL` must validate
//! against `schemas/linear.graphql`, and the variables the builders produce
//! must coerce to the types each operation declares.

mod common;

use std::path::PathBuf;

use apollo_compiler::executable::OperationType;
use apollo_compiler::request::coerce_variable_values;
use apollo_compiler::response::JsonMap;
use apollo_compiler::validation::Valid;
use apollo_compiler::{ExecutableDocument, Schema};
use loupe_linear::documents::{ALL, DocumentKind};

fn schema() -> Valid<Schema> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../schemas/linear.graphql");
    let sdl = std::fs::read_to_string(&path).expect("read schemas/linear.graphql");
    match Schema::parse_and_validate(&sdl, "linear.graphql") {
        Ok(schema) => schema,
        Err(failure) => {
            eprintln!(
                "note: the SDL does not pass strict validation, so it is used as parsed:\n{}",
                failure.errors
            );
            Valid::assume_valid(failure.partial)
        }
    }
}

#[test]
fn every_document_validates_against_the_schema() {
    let schema = schema();
    let mut failures = Vec::new();
    for document in ALL {
        match ExecutableDocument::parse_and_validate(&schema, document.text, document.name) {
            Ok(parsed) => {
                let operation = parsed.operations.get(Some(document.name));
                let Ok(operation) = operation else {
                    failures.push(format!(
                        "{}: the text has no operation named {}",
                        document.name, document.name
                    ));
                    continue;
                };
                let kind = match operation.operation_type {
                    OperationType::Query => DocumentKind::Query,
                    OperationType::Mutation => DocumentKind::Mutation,
                    OperationType::Subscription => {
                        failures.push(format!(
                            "{}: subscriptions are not supported",
                            document.name
                        ));
                        continue;
                    }
                };
                if kind != document.kind {
                    failures.push(format!(
                        "{}: kind says {:?} but the text is {:?}",
                        document.name, document.kind, kind
                    ));
                }
                if parsed.operations.iter().count() != 1 {
                    failures.push(format!("{}: more than one operation", document.name));
                }
            }
            Err(failure) => failures.push(format!("{}:\n{}", document.name, failure.errors)),
        }
    }
    assert!(
        failures.is_empty(),
        "{} document(s) fail schema validation:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}

#[test]
fn document_names_are_unique() {
    let mut names: Vec<&str> = ALL.iter().map(|document| document.name).collect();
    names.sort_unstable();
    let before = names.len();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate document names in ALL");
}

#[test]
fn sample_variables_coerce_to_the_declared_types() {
    let schema = schema();
    let mut failures = Vec::new();
    for document in ALL {
        let parsed = ExecutableDocument::parse_and_validate(&schema, document.text, document.name)
            .unwrap_or_else(|failure| panic!("{} is invalid:\n{}", document.name, failure.errors));
        let operation = parsed
            .operations
            .get(Some(document.name))
            .expect("operation named like its document");
        for (index, variables) in common::sample_variables(document).into_iter().enumerate() {
            let map: JsonMap = serde_json::from_value(variables.clone()).expect("object variables");
            if let Err(error) = coerce_variable_values(&schema, operation, &map) {
                failures.push(format!(
                    "{} (sample {index}): {}\n  variables: {variables}",
                    document.name,
                    error.message()
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "variables do not match the schema:\n\n{}",
        failures.join("\n\n")
    );
}
