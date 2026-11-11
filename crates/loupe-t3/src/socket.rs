//! The WebSocket side of the client: a one-use ticket from the HTTP API,
//! then one socket that carries the requests `rpc` builds, read until each
//! one's `Exit`.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use serde_json::Value;
use tungstenite::client::IntoClientRequest as _;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{HandshakeError, Message, WebSocket};

use crate::rpc;
use crate::{Error, LaunchedReview, Result, ReviewLaunch, T3};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The server answers a launch once it recorded the thread; the worktree
/// is prepared afterwards, in the background.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long one request may take in all, other frames included.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Starts the review thread, then links it to the pull request. A link that
/// fails leaves the thread running, so it comes back with the link's error.
pub(crate) fn launch_review(t3: &T3, launch: &ReviewLaunch) -> Result<LaunchedReview> {
    let mut socket = open(t3)?;
    let value = call(
        &mut socket,
        "1",
        rpc::LAUNCH_THREAD,
        rpc::launch_payload(launch),
    )?;
    let thread_id = rpc::launched_thread_id(&value)?;
    let link = rpc::link_payload(
        &launch.request_id,
        &thread_id,
        &launch.pull_request,
        &launch.pull_request_url,
    );
    let link_error = call(&mut socket, "2", rpc::DISPATCH_COMMAND, link).err();
    let _ = socket.close(None);
    Ok(LaunchedReview {
        thread_id,
        link_error,
    })
}

#[derive(serde::Deserialize)]
struct TicketWire {
    ticket: String,
}

fn ticket(t3: &T3) -> Result<String> {
    let mut response = t3
        .agent
        .post(format!("{}/api/auth/websocket-ticket/rpc", t3.base_url))
        .header("Authorization", format!("Bearer {}", t3.token))
        .send_empty()
        .map_err(|error| Error::Transport(error.to_string()))?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .read_to_vec()
        .map_err(|error| Error::Transport(error.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(crate::http::classify(status, &body));
    }
    let wire: TicketWire =
        serde_json::from_slice(&body).map_err(|error| Error::Decode(error.to_string()))?;
    Ok(wire.ticket)
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn open(t3: &T3) -> Result<Socket> {
    let url = rpc::socket_url(&t3.base_url, &ticket(t3)?);
    let request = url
        .into_client_request()
        .map_err(|error| Error::Transport(error.to_string()))?;
    let uri = request.uri();
    let host = uri
        .host()
        .ok_or_else(|| Error::Transport("the T3 server URL has no host".into()))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });
    let stream = connect(&host, port)?;
    match tungstenite::client_tls_with_config(request, stream, None, None) {
        Ok((socket, _)) => Ok(socket),
        Err(HandshakeError::Failure(error)) => Err(handshake_error(error)),
        Err(HandshakeError::Interrupted(_)) => {
            Err(Error::Transport("the T3 socket handshake timed out".into()))
        }
    }
}

fn connect(host: &str, port: u16) -> Result<TcpStream> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| Error::Transport(error.to_string()))?;
    let mut last = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => {
                let timeouts = stream
                    .set_read_timeout(Some(READ_TIMEOUT))
                    .and_then(|()| stream.set_write_timeout(Some(WRITE_TIMEOUT)));
                timeouts.map_err(|error| Error::Transport(error.to_string()))?;
                return Ok(stream);
            }
            Err(error) => last = Some(error),
        }
    }
    Err(Error::Transport(last.map_or_else(
        || format!("{host} has no address"),
        |error| error.to_string(),
    )))
}

/// The server refuses an upgrade with a plain HTTP status: 401 for a ticket
/// it does not know, 426 for a protocol it does not speak.
fn handshake_error(error: tungstenite::Error) -> Error {
    match error {
        // The ticket was minted a moment ago with a token the server took,
        // so a 401 here is about the ticket, not the token.
        tungstenite::Error::Http(response) if response.status().as_u16() == 401 => {
            Error::Transport("T3 refused the socket ticket; try again".into())
        }
        tungstenite::Error::Http(response) => {
            let body = response.body().as_deref().unwrap_or_default();
            crate::http::classify(response.status().as_u16(), body)
        }
        other => Error::Transport(other.to_string()),
    }
}

/// Sends one request and reads frames until its `Exit`.
fn call<S: Read + Write>(
    socket: &mut WebSocket<S>,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<Value> {
    socket
        .send(Message::text(rpc::request(id, tag, payload)))
        .map_err(transport)?;
    let deadline = Instant::now() + CALL_TIMEOUT;
    loop {
        if Instant::now() > deadline {
            return Err(Error::Transport("T3 did not answer in time".into()));
        }
        let text = match socket.read().map_err(transport)? {
            Message::Text(text) => text,
            Message::Close(_) => {
                return Err(Error::Transport(
                    "T3 closed the socket before it answered".into(),
                ));
            }
            _ => continue,
        };
        if let Some(exit) = rpc::exit(text.as_str(), id) {
            return exit;
        }
    }
}

fn transport(error: tungstenite::Error) -> Error {
    match error {
        tungstenite::Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            Error::Transport("T3 did not answer in time".into())
        }
        other => Error::Transport(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use serde_json::json;
    use tungstenite::protocol::Role;
    use tungstenite::{Message, WebSocket};

    use super::call;
    use crate::Error;

    /// A socket whose incoming side is `incoming`'s frames, as a server
    /// would send them, and whose outgoing side is kept for the test to read.
    struct Duplex {
        incoming: Cursor<Vec<u8>>,
        outgoing: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.incoming.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.outgoing.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn server_frames(frames: &[Message]) -> Vec<u8> {
        let mut server = WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, None);
        for frame in frames {
            server.write(frame.clone()).unwrap();
        }
        server.flush().unwrap();
        server.into_inner().into_inner()
    }

    fn client(frames: &[Message]) -> WebSocket<Duplex> {
        WebSocket::from_raw_socket(
            Duplex {
                incoming: Cursor::new(server_frames(frames)),
                outgoing: Vec::new(),
            },
            Role::Client,
            None,
        )
    }

    #[test]
    fn a_call_skips_other_frames_until_its_exit() {
        let mut socket = client(&[
            Message::Ping(Vec::new().into()),
            Message::text(r#"{"_tag":"Pong"}"#),
            Message::text(r#"{"_tag":"Exit","requestId":"9","exit":{"_tag":"Success","value":0}}"#),
            Message::text(
                r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Success","value":{"threadId":"t-1"}}}"#,
            ),
        ]);
        let value = call(&mut socket, "1", "orchestration.launchThread", json!({})).unwrap();
        assert_eq!(value, json!({"threadId": "t-1"}));
        // The request went out as one masked client frame.
        assert!(!socket.get_ref().outgoing.is_empty());
    }

    #[test]
    fn a_close_before_the_exit_is_an_error() {
        let mut socket = client(&[Message::Close(None)]);
        assert!(matches!(
            call(&mut socket, "1", "orchestration.launchThread", json!({})),
            Err(Error::Transport(_))
        ));
    }

    #[test]
    fn a_failed_exit_is_the_servers_error() {
        let mut socket = client(&[Message::text(
            r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"_tag":"X","message":"Project not found"}}]}}"#,
        )]);
        match call(&mut socket, "1", "orchestration.launchThread", json!({})) {
            Err(Error::Rpc(message)) => assert_eq!(message, "Project not found"),
            other => panic!("expected Rpc, got {other:?}"),
        }
    }
}
