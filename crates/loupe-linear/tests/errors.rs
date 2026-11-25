//! The failures a real HTTP exchange can produce, each classified and worded
//! for the user. The server answers one canned response per connection, so
//! the status line and the body are exactly what Linear's would be.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use loupe_linear::{ErrorKind, Linear};

const BODY_SECRET: &str = "lin_api_BODYSECRET";

fn answer(status: &str, content_type: &str, body: &str) -> Linear {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/graphql", listener.local_addr().expect("addr"));
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                if line.trim().is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':')
                    && name.eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut request = vec![0; length];
            reader.read_exact(&mut request).ok();
            let mut stream = reader.into_inner();
            stream.write_all(response.as_bytes()).ok();
        }
    });
    Linear::with_endpoint("test-key", url)
}

fn graphql_error(status: &str, code: &str, message: &str) -> Linear {
    let body = serde_json::json!({
        "errors": [{ "message": message, "extensions": { "code": code } }],
    });
    answer(status, "application/json", &body.to_string())
}

fn kind_of(linear: &Linear) -> ErrorKind {
    let error = linear.viewer().expect_err("the call must fail");
    let text = error.to_string();
    assert!(!text.contains(BODY_SECRET), "{text}");
    assert!(!text.contains("test-key"), "{text}");
    assert_eq!(text, error.message());
    error.kind()
}

#[test]
fn a_dead_endpoint_is_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}/graphql", listener.local_addr().expect("addr"));
    drop(listener);
    assert_eq!(
        kind_of(&Linear::with_endpoint("test-key", url)),
        ErrorKind::Offline
    );
}

#[test]
fn a_rejected_or_revoked_key_is_told_apart_from_the_network() {
    let by_code = graphql_error("200 OK", "AUTHENTICATION_ERROR", BODY_SECRET);
    assert_eq!(kind_of(&by_code), ErrorKind::KeyRejected);
    let by_status = answer("401 Unauthorized", "text/plain", BODY_SECRET);
    assert_eq!(kind_of(&by_status), ErrorKind::KeyRejected);
}

#[test]
fn a_missing_issue_is_not_found_whichever_way_linear_says_it() {
    let by_message = graphql_error("200 OK", "INVALID_INPUT", "Entity not found: Issue");
    assert_eq!(kind_of(&by_message), ErrorKind::NotFound);
    let by_status = answer("404 Not Found", "application/json", "{}");
    assert_eq!(kind_of(&by_status), ErrorKind::NotFound);
}

#[test]
fn a_key_without_permission_is_forbidden() {
    let by_code = graphql_error("200 OK", "FORBIDDEN", BODY_SECRET);
    assert_eq!(kind_of(&by_code), ErrorKind::Forbidden);
    let by_status = answer("403 Forbidden", "text/plain", BODY_SECRET);
    assert_eq!(kind_of(&by_status), ErrorKind::Forbidden);
}

#[test]
fn a_rate_limit_is_recognised_in_a_400_and_in_a_429() {
    let by_code = graphql_error("400 Bad Request", "RATELIMITED", BODY_SECRET);
    assert_eq!(kind_of(&by_code), ErrorKind::RateLimited);
    let by_status = answer("429 Too Many Requests", "text/plain", BODY_SECRET);
    assert_eq!(kind_of(&by_status), ErrorKind::RateLimited);
}

#[test]
fn a_server_error_is_one_even_when_the_body_is_not_json() {
    let html = format!("<html><body>Bad gateway {BODY_SECRET}</body></html>");
    assert_eq!(
        kind_of(&answer("502 Bad Gateway", "text/html", &html)),
        ErrorKind::Server
    );
    assert_eq!(
        kind_of(&answer("500 Internal Server Error", "application/json", "")),
        ErrorKind::Server
    );
    let by_code = graphql_error("200 OK", "INTERNAL_SERVER_ERROR", BODY_SECRET);
    assert_eq!(kind_of(&by_code), ErrorKind::Server);
}

#[test]
fn anything_else_gets_the_generic_message_without_linears_wording() {
    let refused = graphql_error("200 OK", "INVALID_INPUT", BODY_SECRET);
    assert_eq!(kind_of(&refused), ErrorKind::Other);
    let garbage = answer("200 OK", "text/html", BODY_SECRET);
    assert_eq!(kind_of(&garbage), ErrorKind::Other);
}
