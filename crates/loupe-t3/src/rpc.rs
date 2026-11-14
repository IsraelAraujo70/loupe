//! The messages of T3's WebSocket RPC that Loupe sends to start a thread and
//! link it to a pull request, and how it reads the answer. Pure: the socket
//! itself is in `socket`.
//!
//! A request is one JSON text frame,
//! `{"_tag":"Request","id","tag","payload","headers":[]}`. The server answers
//! with `{"_tag":"Exit","requestId","exit"}`, where `exit` is
//! `{"_tag":"Success","value"}` or `{"_tag":"Failure","cause":[…]}`.

use serde_json::{Value, json};

use crate::{Error, ModelChoice, PullRequestKey, Result, ReviewLaunch};

/// The orchestration protocol this client speaks, as the HTTP header and the
/// socket's query parameter carry it.
pub(crate) const PROTOCOL_VERSION: &str = "2";

pub(crate) const LAUNCH_THREAD: &str = "orchestration.launchThread";
pub(crate) const DISPATCH_COMMAND: &str = "orchestration.dispatchCommand";
/// The server's configuration, whose `providers` list the provider
/// instances and their models. Needs `orchestration:read`.
pub(crate) const GET_CONFIG: &str = "server.getConfig";

/// The review runs the repository's tests on its own, so it starts with
/// every permission the credential can give.
const RUNTIME_MODE: &str = "full-access";

/// The socket's URL for `base_url` (`http` becomes `ws`, `https` `wss`)
/// with a one-use ticket.
pub(crate) fn socket_url(base_url: &str, ticket: &str) -> String {
    let origin = match base_url.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some((_, rest)) => format!("ws://{rest}"),
        None => format!("ws://{base_url}"),
    };
    format!(
        "{origin}/ws?orchestrationProtocol={PROTOCOL_VERSION}&wsTicket={}",
        percent_encode(ticket)
    )
}

fn percent_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// One request as the text of its frame.
pub(crate) fn request(id: &str, tag: &str, payload: Value) -> String {
    json!({ "_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": [] })
        .to_string()
}

fn model_selection(model: &ModelChoice) -> Value {
    let mut selection = json!({ "instanceId": model.instance_id, "model": model.model });
    if let Some(effort) = &model.effort {
        selection["options"] = json!([{ "id": "effort", "value": effort }]);
    }
    selection
}

/// `orchestration.launchThread`: a new thread in the project, in a new
/// worktree on `branch` that starts at the head of `base_ref` as the remote
/// has it, with the review request as its first message.
pub(crate) fn launch_payload(launch: &ReviewLaunch) -> Value {
    json!({
        "commandId": format!("{}:launch", launch.request_id),
        "projectId": launch.project_id,
        "title": launch.title,
        "modelSelection": model_selection(&launch.model),
        "runtimeMode": RUNTIME_MODE,
        "interactionMode": "default",
        "workspaceStrategy": {
            "type": "worktree",
            "baseRef": launch.base_ref,
            "branch": launch.branch,
            "startFromOrigin": true,
        },
        "initialMessage": { "text": launch.message, "attachments": [] },
    })
}

/// `orchestration.dispatchCommand` with the command that links `thread_id`
/// to the pull request, as a person would in T3.
pub(crate) fn link_payload(
    request_id: &str,
    thread_id: &str,
    pr: &PullRequestKey,
    url: &str,
) -> Value {
    json!({
        "type": "thread.pull-request.link",
        "commandId": format!("{request_id}:link"),
        "threadId": thread_id,
        "host": pr.host,
        "repository": pr.repository,
        "number": pr.number,
        "url": url,
        "source": "manual",
    })
}

/// Reads one frame. `None` for a frame that does not end request `id` (a
/// ping, a stream chunk, another request's answer); otherwise the value the
/// request succeeded with, or why it failed.
pub(crate) fn exit(frame: &str, id: &str) -> Option<Result<Value>> {
    let message: Value = serde_json::from_str(frame).ok()?;
    let tag = message.get("_tag").and_then(Value::as_str)?;
    match tag {
        "Exit" if message.get("requestId").and_then(Value::as_str) == Some(id) => {
            let exit = message.get("exit")?;
            Some(match exit.get("_tag").and_then(Value::as_str) {
                Some("Success") => Ok(exit.get("value").cloned().unwrap_or(Value::Null)),
                _ => Err(failure(exit.get("cause"))),
            })
        }
        // The server could not read the request at all.
        "Defect" | "ClientProtocolError" => {
            Some(Err(Error::Rpc(first_text(&message).unwrap_or_else(|| {
                "T3 could not read the request".to_owned()
            }))))
        }
        _ => None,
    }
}

/// The first failure of a cause, as an [`Error`]: a missing scope by name,
/// anything else by the server's own message.
fn failure(cause: Option<&Value>) -> Error {
    let first = cause
        .and_then(Value::as_array)
        .and_then(|list| list.first());
    let error = first.and_then(|entry| entry.get("error"));
    let scope = error
        .and_then(|error| error.get("requiredScope"))
        .and_then(Value::as_str);
    if let Some(scope) = scope {
        return Error::MissingScope(scope.to_owned());
    }
    let message = error
        .and_then(first_text)
        .or_else(|| first.and_then(first_text))
        .unwrap_or_else(|| "T3 refused the request".to_owned());
    Error::Rpc(message)
}

/// The text a failure carries: its `message`, its `defect`, or its tag.
fn first_text(value: &Value) -> Option<String> {
    ["message", "defect", "detail", "_tag"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .map(|text| text.lines().next().unwrap_or(text).to_owned())
}

/// The id of the thread a launch created.
pub(crate) fn launched_thread_id(value: &Value) -> Result<String> {
    value
        .get("threadId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| Error::Decode("the launch answer has no threadId".into()))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::{exit, launch_payload, launched_thread_id, link_payload, request, socket_url};
    use crate::{Error, ModelChoice, PullRequestKey, ReviewLaunch};

    fn launch(effort: Option<&str>) -> ReviewLaunch {
        ReviewLaunch {
            request_id: "loupe-review-1".into(),
            project_id: "project-1".into(),
            title: "Review acme/app#374".into(),
            message: "Review this pull request.".into(),
            model: ModelChoice {
                instance_id: "claude_pool".into(),
                model: "claude-opus-5-5".into(),
                effort: effort.map(str::to_owned),
            },
            base_ref: "feat/thing".into(),
            branch: "review/pr-374".into(),
            pull_request: PullRequestKey::github("acme/app", 374),
            pull_request_url: "https://github.com/acme/app/pull/374".into(),
        }
    }

    #[test]
    fn the_socket_url_swaps_the_scheme_and_carries_the_ticket() {
        assert_eq!(
            socket_url("https://host.ts.net:3774", "a b/c"),
            "wss://host.ts.net:3774/ws?orchestrationProtocol=2&wsTicket=a%20b%2Fc"
        );
        assert_eq!(
            socket_url("http://127.0.0.1:3774", "t"),
            "ws://127.0.0.1:3774/ws?orchestrationProtocol=2&wsTicket=t"
        );
    }

    #[test]
    fn a_request_is_one_json_frame() {
        let frame: Value =
            serde_json::from_str(&request("7", "orchestration.launchThread", json!({"a": 1})))
                .unwrap();
        assert_eq!(
            frame,
            json!({"_tag": "Request", "id": "7", "tag": "orchestration.launchThread",
                   "payload": {"a": 1}, "headers": []})
        );
    }

    #[test]
    fn a_launch_asks_for_a_worktree_at_the_head_with_the_request_as_first_message() {
        assert_eq!(
            launch_payload(&launch(Some("medium"))),
            json!({
                "commandId": "loupe-review-1:launch",
                "projectId": "project-1",
                "title": "Review acme/app#374",
                "modelSelection": {
                    "instanceId": "claude_pool",
                    "model": "claude-opus-5-5",
                    "options": [{"id": "effort", "value": "medium"}],
                },
                "runtimeMode": "full-access",
                "interactionMode": "default",
                "workspaceStrategy": {
                    "type": "worktree",
                    "baseRef": "feat/thing",
                    "branch": "review/pr-374",
                    "startFromOrigin": true,
                },
                "initialMessage": {"text": "Review this pull request.", "attachments": []},
            })
        );
        // Without an effort the model runs at the server's default.
        assert_eq!(
            launch_payload(&launch(None))["modelSelection"],
            json!({"instanceId": "claude_pool", "model": "claude-opus-5-5"})
        );
    }

    #[test]
    fn a_link_names_the_pull_request_the_way_t3_keys_it() {
        let pr = PullRequestKey::github("acme/app", 374);
        assert_eq!(
            link_payload(
                "loupe-review-1",
                "thread-9",
                &pr,
                "https://github.com/acme/app/pull/374"
            ),
            json!({
                "type": "thread.pull-request.link",
                "commandId": "loupe-review-1:link",
                "threadId": "thread-9",
                "host": "github.com",
                "repository": "acme/app",
                "number": 374,
                "url": "https://github.com/acme/app/pull/374",
                "source": "manual",
            })
        );
    }

    #[test]
    fn a_success_gives_its_value_and_the_thread_id() {
        let frame = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Success","value":{"threadId":"t-1","resumed":false}}}"#;
        let value = exit(frame, "1").expect("the exit").expect("success");
        assert_eq!(launched_thread_id(&value).unwrap(), "t-1");
        assert!(matches!(
            launched_thread_id(&json!({})),
            Err(Error::Decode(_))
        ));
    }

    /// The frame the real server sent a read-only token on 2026-11-11.
    #[test]
    fn a_token_without_the_scope_is_a_missing_scope() {
        let frame = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"EnvironmentAuthorizationError","message":"The authenticated token is missing required scope: orchestration:operate.","requiredScope":"orchestration:operate","requiredPermission":"orchestration:operate"}}]}}"#;
        match exit(frame, "1") {
            Some(Err(Error::MissingScope(scope))) => assert_eq!(scope, "orchestration:operate"),
            other => panic!("expected MissingScope, got {other:?}"),
        }
    }

    /// The frame the real server sent for a payload it could not decode.
    #[test]
    fn other_failures_carry_the_servers_first_line() {
        let frame = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Die","defect":"Expected string\n  at [\"modelSelection\"][\"instanceId\"]"}]}}"#;
        match exit(frame, "1") {
            Some(Err(Error::Rpc(message))) => assert_eq!(message, "Expected string"),
            other => panic!("expected Rpc, got {other:?}"),
        }
        let named = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"OrchestrationV2ThreadLaunchError","message":"Base ref not found: feat/x"}}]}}"#;
        match exit(named, "1") {
            Some(Err(Error::Rpc(message))) => assert_eq!(message, "Base ref not found: feat/x"),
            other => panic!("expected Rpc, got {other:?}"),
        }
    }

    #[test]
    fn frames_that_do_not_end_the_request_are_skipped() {
        assert!(exit(r#"{"_tag":"Pong"}"#, "1").is_none());
        assert!(exit(r#"{"_tag":"Chunk","requestId":"1","values":[]}"#, "1").is_none());
        let other = r#"{"_tag":"Exit","requestId":"2","exit":{"_tag":"Success","value":1}}"#;
        assert!(exit(other, "1").is_none());
        assert!(exit("not json", "1").is_none());
    }
}
