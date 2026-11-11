//! GPUI's HTTP client, backed by `ureq` on background threads, so `img(url)`
//! and `ui::avatar` load remote images (GitHub and Linear avatars, markdown
//! images). Linear's upload host answers 401 without the API key, so requests
//! to it carry the connected key; ureq drops it on any redirect.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use futures::AsyncReadExt as _;
use futures::channel::oneshot;
use futures::future::BoxFuture;
use gpui_kit::App;
use gpui_kit::http_client::http::header::AUTHORIZATION;
use gpui_kit::http_client::http::{HeaderValue, Request, Response, Uri};
use gpui_kit::http_client::{AsyncBody, HttpClient, RedirectPolicy, RequestTimeout, Url};
use ureq::RequestExt as _;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest response body accepted, so a bad URL cannot exhaust memory.
const MAX_BODY_BYTES: u64 = 50 * 1024 * 1024;
const WORKER_THREADS: usize = 4;
/// What "follow all redirects" means in practice: ureq needs a finite bound.
const FOLLOW_ALL_LIMIT: u32 = 20;

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Where Linear serves files attached to issues and comments.
const LINEAR_UPLOAD_HOST: &str = "uploads.linear.app";

/// The connected Linear API key, for [`LINEAR_UPLOAD_HOST`] only.
static LINEAR_UPLOAD_KEY: RwLock<Option<String>> = RwLock::new(None);

/// Sets or clears the key sent to Linear's upload host. `settings::session`
/// calls it whenever the Linear client changes.
pub fn set_linear_upload_key(key: Option<String>) {
    if let Ok(mut slot) = LINEAR_UPLOAD_KEY.write() {
        *slot = key;
    }
}

/// The `Authorization` value for `uri`: the Linear key over https to the
/// upload host, nothing for any other URL.
fn upload_authorization(uri: &Uri) -> Option<HeaderValue> {
    if uri.scheme_str() != Some("https") || uri.host() != Some(LINEAR_UPLOAD_HOST) {
        return None;
    }
    let key = LINEAR_UPLOAD_KEY.read().ok()?.clone()?;
    let mut value = HeaderValue::from_str(&key).ok()?;
    value.set_sensitive(true);
    Some(value)
}

/// Installs the client with `cx.set_http_client`. Call once at startup.
pub fn install(cx: &mut App) {
    cx.set_http_client(Arc::new(UreqHttpClient::new()));
}

/// Blocking `ureq` calls run on a small fixed pool of worker threads, so
/// `send` never blocks the caller and a page of avatars cannot spawn a thread
/// per image.
struct UreqHttpClient {
    agent: ureq::Agent,
    user_agent: HeaderValue,
    jobs: Sender<Job>,
}

impl UreqHttpClient {
    fn new() -> Self {
        let user_agent_text = format!("Loupe/{}", env!("CARGO_PKG_VERSION"));
        let user_agent = HeaderValue::from_str(&user_agent_text)
            .unwrap_or_else(|_| HeaderValue::from_static("Loupe"));
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(user_agent_text)
            .build()
            .into();

        let (jobs, queue) = mpsc::channel::<Job>();
        let queue: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(queue));
        for _ in 0..WORKER_THREADS {
            let queue = queue.clone();
            std::thread::Builder::new()
                .name("loupe-http".into())
                .spawn(move || {
                    loop {
                        let job = match queue.lock() {
                            Ok(queue) => queue.recv(),
                            Err(_) => return,
                        };
                        match job {
                            Ok(job) => job(),
                            Err(_) => return,
                        }
                    }
                })
                .ok();
        }

        Self {
            agent,
            user_agent,
            jobs,
        }
    }
}

impl HttpClient for UreqHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(
        &self,
        request: Request<AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let agent = self.agent.clone();
        let jobs = self.jobs.clone();
        Box::pin(async move {
            let (mut parts, mut body) = request.into_parts();
            if !parts.headers.contains_key(AUTHORIZATION)
                && let Some(value) = upload_authorization(&parts.uri)
            {
                parts.headers.insert(AUTHORIZATION, value);
            }
            let mut payload = Vec::new();
            body.read_to_end(&mut payload)
                .await
                .context("reading request body")?;

            let redirects = match parts.extensions.get::<RedirectPolicy>() {
                None | Some(RedirectPolicy::NoFollow) => 0,
                Some(RedirectPolicy::FollowLimit(limit)) => *limit,
                Some(RedirectPolicy::FollowAll) => FOLLOW_ALL_LIMIT,
            };
            let timeout = parts
                .extensions
                .get::<RequestTimeout>()
                .map_or(DEFAULT_TIMEOUT, |timeout| timeout.0);
            let request = Request::from_parts(parts, payload);

            let (reply, response) = oneshot::channel();
            let job: Job = Box::new(move || {
                reply
                    .send(execute(&agent, request, redirects, timeout))
                    .ok();
            });
            jobs.send(job)
                .map_err(|_| anyhow!("the HTTP worker threads are gone"))?;
            response
                .await
                .map_err(|_| anyhow!("the HTTP request was dropped before it finished"))?
        })
    }
}

/// Runs one request to completion on the calling (worker) thread.
fn execute(
    agent: &ureq::Agent,
    request: Request<Vec<u8>>,
    redirects: u32,
    timeout: Duration,
) -> anyhow::Result<Response<AsyncBody>> {
    let uri = request.uri().to_string();
    let response = if request.body().is_empty() {
        let (parts, _) = request.into_parts();
        send_with(agent, Request::from_parts(parts, ()), redirects, timeout)
    } else {
        send_with(agent, request, redirects, timeout)
    }
    .with_context(|| format!("requesting {uri}"))?;

    let (parts, mut body) = response.into_parts();
    let bytes = body
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_vec()
        .with_context(|| format!("reading the response from {uri}"))?;
    Ok(Response::from_parts(parts, AsyncBody::from(bytes)))
}

fn send_with<B: ureq::AsSendBody>(
    agent: &ureq::Agent,
    request: Request<B>,
    redirects: u32,
    timeout: Duration,
) -> Result<Response<ureq::Body>, ureq::Error> {
    request
        .with_agent(agent)
        .configure()
        .max_redirects(redirects)
        .max_redirects_will_error(false)
        .timeout_global(Some(timeout))
        .run()
}

#[cfg(test)]
mod tests {
    use super::UreqHttpClient;
    use futures::AsyncReadExt as _;
    use gpui_kit::http_client::{
        AsyncBody, HttpClient as _, HttpRequestExt as _, RedirectPolicy, Request,
    };
    use std::time::Duration;

    const PNG: &[u8] = &[0x89, b'P', b'N', b'G'];
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff];

    #[test]
    fn sends_the_linear_key_only_to_the_upload_host() {
        let auth = |url: &str| {
            super::upload_authorization(&url.parse().expect("a valid URL"))
                .map(|value| value.to_str().expect("an ASCII key").to_owned())
        };
        super::set_linear_upload_key(Some("lin_api_dummy".into()));
        assert_eq!(
            auth("https://uploads.linear.app/org/issue/file").as_deref(),
            Some("lin_api_dummy")
        );
        assert_eq!(auth("http://uploads.linear.app/org/issue/file"), None);
        assert_eq!(auth("https://uploads.linear.app.example.com/file"), None);
        assert_eq!(auth("https://example.com/uploads.linear.app/file"), None);
        assert_eq!(auth("https://avatars.githubusercontent.com/u/1"), None);
        super::set_linear_upload_key(None);
        assert_eq!(auth("https://uploads.linear.app/org/issue/file"), None);
    }

    #[test]
    fn loads_a_public_avatar_through_redirects() {
        let client = UreqHttpClient::new();
        let request = Request::get("https://avatars.githubusercontent.com/u/9919?v=4")
            .follow_redirects(RedirectPolicy::FollowAll)
            .timeout(Duration::from_secs(30))
            .body(AsyncBody::empty())
            .expect("valid request");

        futures::executor::block_on(async {
            let mut response = client.send(request).await.expect("avatar request");
            assert_eq!(response.status(), 200);
            let mut body = Vec::new();
            response
                .body_mut()
                .read_to_end(&mut body)
                .await
                .expect("avatar body");
            assert!(
                body.starts_with(PNG) || body.starts_with(JPEG),
                "body is not a PNG or JPEG ({} bytes)",
                body.len()
            );
        });
    }

    #[test]
    fn an_unroutable_host_is_an_error_not_a_panic() {
        let client = UreqHttpClient::new();
        let request = Request::get("http://127.0.0.1:9/")
            .body(AsyncBody::empty())
            .expect("valid request");
        let result = futures::executor::block_on(client.send(request));
        assert!(result.is_err());
    }

    #[test]
    fn redirects_are_not_followed_by_default() {
        let client = UreqHttpClient::new();
        // github.com answers a profile image URL with a 302 to the avatar CDN.
        let request = Request::get("https://github.com/github.png")
            .body(AsyncBody::empty())
            .expect("valid request");
        let response = futures::executor::block_on(client.send(request)).expect("request");
        assert!(
            response.status().is_redirection(),
            "got {}",
            response.status()
        );
    }
}
