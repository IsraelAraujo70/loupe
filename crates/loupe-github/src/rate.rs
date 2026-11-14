//! GitHub's rate limits, as its responses report them in their headers.

use chrono::DateTime;
use ureq::http::HeaderMap;

use crate::Timestamp;

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

    use super::{RateLimit, RateLimits};

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
