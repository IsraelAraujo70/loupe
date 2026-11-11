//! Live validation gate. Sends every document to Linear without credentials.
//! Linear validates a document before it authenticates the request, so a
//! valid document answers `AUTHENTICATION_ERROR` and an invalid one answers
//! `GRAPHQL_VALIDATION_FAILED`. Nothing is read or changed.
//!
//!     cargo test -p loupe-linear --test live_validation -- --ignored --nocapture

mod common;

use std::time::Duration;

use loupe_linear::documents::ALL;
use serde_json::{Value, json};

const ENDPOINT: &str = "https://api.linear.app/graphql";

fn error_codes(body: &Value) -> Vec<String> {
    body["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .map(|error| {
                    error["extensions"]["code"]
                        .as_str()
                        .unwrap_or("(no code)")
                        .to_owned()
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
#[ignore = "calls api.linear.app without credentials"]
fn every_document_passes_linear_validation() {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut failures = Vec::new();
    for document in ALL {
        for (index, variables) in common::sample_variables(document).into_iter().enumerate() {
            let sent = agent.post(ENDPOINT).send_json(json!({
                "query": document.text,
                "operationName": document.name,
                "variables": variables,
            }));
            let verdict = match sent {
                Err(error) => Err(format!("transport error: {error}")),
                Ok(mut response) => {
                    let status = response.status().as_u16();
                    match response.body_mut().read_json::<Value>() {
                        Err(error) => Err(format!("HTTP {status}, unreadable body: {error}")),
                        Ok(body) => {
                            let codes = error_codes(&body);
                            if codes.iter().any(|code| code == "GRAPHQL_VALIDATION_FAILED") {
                                Err(format!("HTTP {status}: {}", body["errors"]))
                            } else if codes.iter().any(|code| code == "AUTHENTICATION_ERROR") {
                                Ok(format!("HTTP {status} {}", codes.join(",")))
                            } else {
                                Err(format!("HTTP {status}: unexpected codes {codes:?}: {body}"))
                            }
                        }
                    }
                }
            };
            match verdict {
                Ok(note) => println!("ok    {:<22} sample {index}  {note}", document.name),
                Err(reason) => {
                    println!("FAIL  {:<22} sample {index}  {reason}", document.name);
                    failures.push(format!("{} (sample {index}): {reason}", document.name));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "Linear rejected {} request(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
