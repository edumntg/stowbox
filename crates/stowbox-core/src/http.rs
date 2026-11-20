//! What the providers' REST clients share: reading a bounded answer, and
//! when a refusal is worth asking again.

use std::time::Duration;

use reqwest::Method;

use crate::{Error, Result};

/// The whole body, refusing one longer than `cap`. `who` names the
/// provider in the error.
pub async fn read_capped(mut resp: reqwest::Response, cap: usize, who: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if out.len() + chunk.len() > cap {
            return Err(Error::Provider(format!("{who} answered with more than {cap} bytes")));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Retry-After past this is not waited for: the call fails instead.
pub const MAX_WAIT: Duration = Duration::from_secs(8);
/// Retries after the first attempt.
pub const RETRIES: u32 = 2;

/// A Retry-After in seconds. A value too large for a `Duration` saturates,
/// so it reads as "not soon" instead of panicking.
pub fn seconds(s: f64) -> Option<Duration> {
    (s.is_finite() && s >= 0.0).then(|| Duration::try_from_secs_f64(s).unwrap_or(Duration::MAX))
}

/// The Retry-After header, in its seconds form.
pub fn header(resp: &reqwest::Response) -> Option<Duration> {
    resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<f64>().ok()).and_then(seconds)
}

/// The wait before asking again, or `None` to give up. A 429 is refused
/// before anything runs, so any request retries on it; a 503 may come from
/// a proxy after the backend acted, so only requests that are safe to repeat
/// retry on it.
pub fn wait(attempt: u32, method: &Method, status: u16, retry_after: Option<Duration>) -> Option<Duration> {
    let again = match status {
        429 => true,
        503 => matches!(*method, Method::GET | Method::HEAD | Method::DELETE | Method::PUT),
        _ => false,
    };
    let soon = retry_after.is_none_or(|d| d <= MAX_WAIT);
    (attempt < RETRIES && again && soon).then(|| retry_after.unwrap_or(Duration::from_secs(1 << (attempt + 1))).min(MAX_WAIT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_huge_retry_after_means_do_not_wait() {
        assert_eq!(seconds(1e300), Some(Duration::MAX));
        assert_eq!(wait(0, &Method::GET, 429, seconds(1e300)), None);
        assert_eq!(seconds(f64::NAN), None);
        assert_eq!(seconds(-1.0), None);
        assert_eq!(seconds(2.5), Some(Duration::from_millis(2500)));
    }

    #[test]
    fn a_post_retries_on_429_but_not_on_503() {
        assert_eq!(wait(0, &Method::POST, 429, None), Some(Duration::from_secs(2)));
        assert_eq!(wait(0, &Method::POST, 503, None), None);
        assert_eq!(wait(1, &Method::GET, 503, Some(Duration::from_secs(1))), Some(Duration::from_secs(1)));
        assert_eq!(wait(RETRIES, &Method::GET, 429, None), None);
        assert_eq!(wait(0, &Method::GET, 500, None), None);
    }
}
