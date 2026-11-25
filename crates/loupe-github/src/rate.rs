//! GitHub's rate limits, as its responses report them in their headers.

use chrono::{DateTime, Utc};
use ureq::http::HeaderMap;

use crate::{Error, Timestamp};

/// One rate-limit bucket as the latest response that used it reported it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimit {
    pub limit: u32,
    pub remaining: u32,
    /// When the bucket fills up again.
    pub reset: Timestamp,
}

impl RateLimit {
    /// Under a tenth of the bucket left: refreshes nobody asked for wait for
    /// the reset, so what the user does next still has calls to spend.
    pub fn is_low(&self) -> bool {
        u64::from(self.remaining) * 10 < u64::from(self.limit)
    }
}

/// The REST (`core`) and GraphQL buckets; `None` until a response used one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RateLimits {
    pub core: Option<RateLimit>,
    pub graphql: Option<RateLimit>,
}

impl RateLimits {
    /// Keeps what `headers` report about their bucket. Buckets other than
    /// `core` and `graphql` (search, for one) are left out.
    pub(crate) fn record(&mut self, headers: &HeaderMap) {
        let Some((resource, limit)) = parse(headers) else {
            return;
        };
        match resource.as_str() {
            "core" => self.core = Some(limit),
            "graphql" => self.graphql = Some(limit),
            _ => {}
        }
    }
}

/// The error a refused call stands for when GitHub refused it for a rate
/// limit: a 429, or a 403 that says so with an empty `x-ratelimit-remaining`,
/// a `retry-after`, or its message. `reset` comes from `retry-after` (a
/// secondary limit) or else from `x-ratelimit-reset`.
pub(crate) fn refusal(status: u16, headers: &HeaderMap, message: &str) -> Option<Error> {
    let number = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
    let retry_after = number("retry-after");
    let used_up = number("x-ratelimit-remaining") == Some(0);
    let said_so = message.to_ascii_lowercase().contains("rate limit");
    if status != 429 && !(status == 403 && (used_up || retry_after.is_some() || said_so)) {
        return None;
    }
    Some(Error::RateLimited {
        reset: reset_time(headers, Utc::now()),
    })
}

/// When the refused bucket opens again: `retry-after` seconds from `now`, or
/// else the `x-ratelimit-reset` epoch.
fn reset_time(headers: &HeaderMap, now: Timestamp) -> Option<Timestamp> {
    let number = |name: &str| headers.get(name)?.to_str().ok()?.trim().parse::<i64>().ok();
    match number("retry-after") {
        Some(seconds) => now.checked_add_signed(chrono::Duration::seconds(seconds)),
        None => DateTime::from_timestamp(number("x-ratelimit-reset")?, 0),
    }
}

/// The error a 200 GraphQL answer stands for when its `errors` say a rate
/// limit is used up (GitHub answers a spent GraphQL bucket this way).
pub(crate) fn graphql_refusal(body: &serde_json::Value, headers: &HeaderMap) -> Option<Error> {
    body["errors"]
        .as_array()?
        .iter()
        .any(|error| error["type"] == "RATE_LIMITED")
        .then(|| Error::RateLimited {
            reset: reset_time(headers, Utc::now()),
        })
}

fn parse(headers: &HeaderMap) -> Option<(String, RateLimit)> {
    let text = |name: &str| headers.get(name)?.to_str().ok();
    let number = |name: &str| text(name)?.trim().parse::<u64>().ok();
    let reset = DateTime::from_timestamp(i64::try_from(number("x-ratelimit-reset")?).ok()?, 0)?;
    Some((
        text("x-ratelimit-resource")?.trim().to_ascii_lowercase(),
        RateLimit {
            limit: u32::try_from(number("x-ratelimit-limit")?).ok()?,
            remaining: u32::try_from(number("x-ratelimit-remaining")?).ok()?,
            reset,
        },
    ))
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;
    use ureq::http::{HeaderMap, HeaderValue};

    use super::{RateLimit, RateLimits, graphql_refusal, refusal};
    use crate::Error;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        map
    }

    #[test]
    fn records_each_bucket_from_the_headers_of_its_responses() {
        let mut limits = RateLimits::default();
        limits.record(&headers(&[
            ("x-ratelimit-resource", "core"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "3687"),
            ("x-ratelimit-reset", "1794415931"),
        ]));
        limits.record(&headers(&[
            ("x-ratelimit-resource", "graphql"),
            ("x-ratelimit-limit", "5000"),
            ("x-ratelimit-remaining", "12"),
            ("x-ratelimit-reset", "1794416000"),
        ]));
        assert_eq!(
            limits.core,
            Some(RateLimit {
                limit: 5000,
                remaining: 3687,
                reset: DateTime::from_timestamp(1_794_415_931, 0).unwrap(),
            })
        );
        assert_eq!(limits.graphql.map(|limit| limit.remaining), Some(12));
    }

    #[test]
    fn other_buckets_and_partial_headers_change_nothing() {
        let mut limits = RateLimits::default();
        limits.record(&headers(&[
            ("x-ratelimit-resource", "search"),
            ("x-ratelimit-limit", "30"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "1794415931"),
        ]));
        limits.record(&headers(&[
            ("x-ratelimit-resource", "core"),
            ("x-ratelimit-remaining", "10"),
        ]));
        assert_eq!(limits, RateLimits::default());
    }

    fn reset_of(error: Option<Error>) -> Option<Option<i64>> {
        match error? {
            Error::RateLimited { reset } => Some(reset.map(|reset| reset.timestamp())),
            other => panic!("not a rate limit: {other}"),
        }
    }

    #[test]
    fn a_403_with_an_empty_bucket_resets_when_the_header_says() {
        let refused = refusal(
            403,
            &headers(&[
                ("x-ratelimit-remaining", "0"),
                ("x-ratelimit-reset", "1794415931"),
            ]),
            "API rate limit exceeded for user ID 1.",
        );
        assert_eq!(reset_of(refused), Some(Some(1_794_415_931)));
    }

    #[test]
    fn a_secondary_limit_waits_retry_after_seconds() {
        let before = chrono::Utc::now().timestamp();
        let refused = refusal(
            429,
            &headers(&[("retry-after", "60"), ("x-ratelimit-remaining", "4000")]),
            "",
        );
        let reset = reset_of(refused).unwrap().unwrap();
        assert!((before + 60..=before + 62).contains(&reset));
    }

    #[test]
    fn a_403_that_names_the_limit_without_headers_has_no_reset() {
        let refused = refusal(
            403,
            &headers(&[]),
            "You have exceeded a secondary rate limit.",
        );
        assert_eq!(reset_of(refused), Some(None));
    }

    #[test]
    fn other_refusals_are_not_rate_limits() {
        let empty = headers(&[("x-ratelimit-remaining", "4000")]);
        assert!(refusal(403, &empty, "Resource not accessible by integration").is_none());
        assert!(refusal(404, &empty, "Not Found").is_none());
        assert!(refusal(502, &empty, "").is_none());
    }

    #[test]
    fn a_graphql_rate_limited_error_resets_with_the_headers() {
        let body = serde_json::json!({
            "errors": [{ "type": "RATE_LIMITED", "message": "API rate limit exceeded" }]
        });
        let refused = graphql_refusal(&body, &headers(&[("x-ratelimit-reset", "1794415931")]));
        assert_eq!(reset_of(refused), Some(Some(1_794_415_931)));
        let other = serde_json::json!({ "errors": [{ "type": "NOT_FOUND", "message": "x" }] });
        assert!(graphql_refusal(&other, &headers(&[])).is_none());
        assert!(graphql_refusal(&serde_json::json!({ "data": {} }), &headers(&[])).is_none());
    }

    #[test]
    fn a_bucket_is_low_under_a_tenth_of_its_limit() {
        let limit = |remaining| RateLimit {
            limit: 5000,
            remaining,
            reset: DateTime::from_timestamp(0, 0).unwrap(),
        };
        assert!(!limit(500).is_low());
        assert!(limit(499).is_low());
        assert!(limit(0).is_low());
    }
}
