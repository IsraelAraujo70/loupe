//! The connected workspace's logo, for the sidebar's mark. Linear names it
//! in `organization.logoUrl` and serves it from its upload host, which
//! answers only with the API key; the app's HTTP client sends the key to
//! that host and to no other (`shell::http`), so the image is read through
//! it and the key never passes here.
//!
//! The bytes are kept in the snapshot cache per workspace: the logo is on
//! screen at once on the next start, and with the app offline. It is read
//! again once per run, in case the workspace changed it.

use std::sync::Arc;

use futures::AsyncReadExt as _;
use gpui_kit::http_client::http::Request;
use gpui_kit::http_client::{AsyncBody, HttpClient};
use gpui_kit::{App, Global, Image, ImageFormat, Task};
use serde::{Deserialize, Serialize};

use crate::services::{Accounts, Connection};

/// A logo is a small square image: anything larger is not read.
const MAX_BYTES: usize = 1024 * 1024;

/// What the cache keeps for a workspace: the image and the URL it came
/// from, so a new URL is never shown with the old image.
#[derive(Serialize, Deserialize)]
struct Stored {
    url: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct WorkspaceLogo {
    /// The workspace id and logo URL the image below belongs to.
    source: Option<(String, String)>,
    image: Option<Arc<Image>>,
    _refresh: Option<Task<()>>,
}

impl Global for WorkspaceLogo {}

fn cache_key(organization_id: &str) -> String {
    format!("linear/{organization_id}/logo")
}

/// The format of an image, by its first bytes: the ones Linear serves a
/// logo in. `None` for anything else, which is then not drawn.
fn format_of(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(ImageFormat::Gif)
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

fn decode(bytes: Vec<u8>) -> Option<Arc<Image>> {
    format_of(&bytes).map(|format| Arc::new(Image::from_bytes(format, bytes)))
}

/// The cached image for `url`, when the cache holds one for that URL.
fn cached(organization_id: &str, url: &str) -> Option<Vec<u8>> {
    crate::cache::get::<Stored>(&cache_key(organization_id))
        .filter(|stored| stored.url == url)
        .map(|stored| stored.bytes)
}

async fn fetch(client: Arc<dyn HttpClient>, url: &str) -> anyhow::Result<Vec<u8>> {
    let request = Request::get(url).body(AsyncBody::empty())?;
    let mut response = client.send(request).await?;
    anyhow::ensure!(response.status().is_success(), "HTTP {}", response.status());
    let mut bytes = Vec::new();
    response
        .body_mut()
        .take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    anyhow::ensure!(bytes.len() <= MAX_BYTES, "the image is too large");
    Ok(bytes)
}

/// Reads the logo again and, when it is new or changed, caches and shows
/// it. A failed read keeps what is on screen.
fn refresh(organization_id: String, url: String, known: Option<Vec<u8>>, cx: &mut App) -> Task<()> {
    let client = cx.http_client();
    cx.spawn(async move |cx| {
        let Ok(bytes) = fetch(client, &url).await else {
            return;
        };
        if known.as_deref() == Some(bytes.as_slice()) {
            return;
        }
        let Some(image) = decode(bytes.clone()) else {
            return;
        };
        crate::cache::put(
            &cache_key(&organization_id),
            &Stored {
                url: url.clone(),
                bytes,
            },
        );
        cx.update(|cx| {
            let state = cx.global_mut::<WorkspaceLogo>();
            // Another workspace may have connected since.
            if state.source == Some((organization_id, url)) {
                state.image = Some(image);
                cx.refresh_windows();
            }
        });
    })
}

/// The connected workspace's logo; `None` while Linear is not connected,
/// when the workspace has no logo, and until a first read of it arrives.
pub fn image(cx: &mut App) -> Option<Arc<Image>> {
    let source = match &Accounts::get(cx).linear {
        Connection::Connected(viewer) => {
            let organization = &viewer.organization;
            organization
                .logo_url
                .clone()
                .map(|url| (organization.id.clone(), url))
        }
        _ => None,
    };
    if !cx.has_global::<WorkspaceLogo>() {
        cx.set_global(WorkspaceLogo::default());
    }
    if cx.global::<WorkspaceLogo>().source != source {
        let known = source
            .as_ref()
            .and_then(|(organization_id, url)| cached(organization_id, url));
        let image = known.clone().and_then(decode);
        // Offline, the app makes no request: the cached logo is the logo.
        let online = std::env::var_os("LOUPE_OFFLINE").is_none();
        let refresh = source
            .clone()
            .filter(|_| online)
            .map(|(organization_id, url)| refresh(organization_id, url, known, cx));
        cx.set_global(WorkspaceLogo {
            source,
            image,
            _refresh: refresh,
        });
    }
    cx.global::<WorkspaceLogo>().image.clone()
}

#[cfg(test)]
mod tests {
    use gpui_kit::ImageFormat;

    use super::{Stored, cache_key, cached, format_of};

    #[test]
    fn a_logo_is_recognized_by_its_first_bytes() {
        assert_eq!(
            format_of(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR"),
            Some(ImageFormat::Png)
        );
        assert_eq!(
            format_of(b"\xff\xd8\xff\xe0\x00\x10JFIF"),
            Some(ImageFormat::Jpeg)
        );
        assert_eq!(format_of(b"GIF89a\x01\x00"), Some(ImageFormat::Gif));
        assert_eq!(
            format_of(b"RIFF\x24\x00\x00\x00WEBPVP8 "),
            Some(ImageFormat::Webp)
        );
        // An error page, an empty answer, a cut header: not drawn.
        for other in [
            &b"<!doctype html>"[..],
            b"",
            b"\x89PN",
            b"RIFF\x24\x00\x00\x00WAVE",
        ] {
            assert_eq!(format_of(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_cache_gives_a_workspaces_logo_back_only_for_the_url_it_came_from() {
        let organization = "org-cache-test";
        let url = "https://uploads.linear.app/org/a/256x256/logo";
        assert_eq!(cached(organization, url), None, "nothing cached yet");
        crate::cache::put(
            &cache_key(organization),
            &Stored {
                url: url.into(),
                bytes: b"\x89PNG\r\n\x1a\nrest".to_vec(),
            },
        );
        assert_eq!(
            cached(organization, url).as_deref(),
            Some(&b"\x89PNG\r\n\x1a\nrest"[..])
        );
        // The workspace uploaded another logo: the old image is not it.
        assert_eq!(
            cached(
                organization,
                "https://uploads.linear.app/org/b/256x256/logo"
            ),
            None
        );
        assert_eq!(cached("another-org", url), None);
        crate::cache::remove(&cache_key(organization));
    }
}
