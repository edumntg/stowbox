//! boat.dev's REST API: bearer auth, the `{ ok, type, … }` envelope, the
//! error envelope, `Retry-After` on 429 and `retryAfterSeconds` on 503.

use crate::{Error, Result};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::time::Duration;

/// How long one wait on a 429 or 503 may be before giving up: a minute,
/// hour or day limit is not worth sleeping through inside a click.
const MAX_WAIT: Duration = Duration::from_secs(8);
const RETRIES: u32 = 2;

pub struct Api {
    http: reqwest::Client,
    base: String,
    key: String,
}

/// One refused call, as boat described it.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub code: String,
    pub message: String,
    pub request_id: String,
    pub retry_after: Option<Duration>,
}

impl Refusal {
    /// Reads boat's error envelope; anything else keeps the HTTP status and
    /// the first line of the body.
    pub fn parse(status: u16, retry_header: Option<&str>, body: &str) -> Self {
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or("").to_string();
        let code = Some(s("/code")).filter(|c| !c.is_empty()).unwrap_or_else(|| s("/error/code"));
        let mut message = Some(s("/message")).filter(|c| !c.is_empty()).unwrap_or_else(|| s("/error/message"));
        if message.is_empty() {
            message = body.lines().next().unwrap_or("").chars().take(200).collect();
        }
        let secs = |p: &str| v.pointer(p).and_then(Value::as_f64);
        let retry_after = retry_header
            .and_then(|h| h.trim().parse::<f64>().ok())
            .or_else(|| secs("/retryAfterSeconds"))
            .or_else(|| secs("/retryAfter"))
            .or_else(|| secs("/error/details/retryAfterSeconds"))
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(Duration::from_secs_f64);
        Refusal { status, code, message, request_id: s("/requestId"), retry_after }
    }

    fn retryable(&self) -> bool {
        // Start and rate limits answer 429; a busy region 503. Both say when.
        matches!(self.status, 429 | 503) && self.retry_after.map(|d| d <= MAX_WAIT).unwrap_or(true)
    }

    pub fn into_error(self) -> Error {
        let what = if self.code.is_empty() { self.message.clone() } else if self.message.is_empty() { self.code.clone() } else { format!("{} ({})", self.message, self.code) };
        match self.status {
            401 => Error::Auth(what),
            403 => Error::Auth(format!("refused: {what}")),
            404 => Error::NotFound(what),
            429 if self.code == "rate_limited" || self.code == "IP_RATE_LIMIT_EXCEEDED" => Error::RateLimited,
            _ => Error::Provider(format!("boat: {what}")),
        }
    }
}

impl Api {
    pub fn new(base: &str, key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), key })
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None, &[]).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body), &[]).await
    }

    /// A POST that makes something billable (create, fork): the same key on
    /// a retry returns the first result instead of a second box.
    pub async fn post_once(&self, path: &str, body: Value) -> Result<Value> {
        let key = uuid::Uuid::new_v4().to_string();
        self.call(Method::POST, path, Some(body), &[("Idempotency-Key", key.as_str())]).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body), &[]).await
    }

    pub async fn delete(&self, path: &str, headers: &[(&str, &str)]) -> Result<Value> {
        self.call(Method::DELETE, path, None, headers).await
    }

    /// A URL the API handed out, made absolute against the API's origin
    /// when it came as a path.
    pub fn absolute(&self, url: &str) -> String {
        if url.starts_with("https://") || url.starts_with("http://") {
            return url.to_string();
        }
        let after_scheme = self.base.find("://").map(|i| i + 3).unwrap_or(0);
        let origin = match self.base[after_scheme..].find('/') {
            Some(i) => &self.base[..after_scheme + i],
            None => &self.base,
        };
        format!("{origin}/{}", url.trim_start_matches('/'))
    }

    /// PUTs raw bytes to a signed URL. The URL carries its own grant, so no
    /// bearer goes with it, and it is kept out of every error: anyone who
    /// has it can write the file until it expires.
    pub async fn put_signed(&self, url: &str, data: Vec<u8>) -> Result<()> {
        let size = data.len() as u64;
        let resp = self
            .http
            .put(url)
            .header("Content-Type", "application/octet-stream")
            .timeout(transfer_timeout(size))
            .body(data)
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        let r = Refusal::parse(status.as_u16(), None, &text);
        let what = [r.message, r.code].into_iter().find(|s| !s.is_empty()).unwrap_or_default();
        Err(Error::Provider(format!("boat refused the upload ({}){}", status.as_u16(), if what.is_empty() { String::new() } else { format!(": {what}") })))
    }

    /// Every item of a cursor-paged list, `field` being the array's name.
    pub async fn list_all(&self, path: &str, field: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        let sep = if path.contains('?') { '&' } else { '?' };
        // A guard against a server that keeps saying "more" forever.
        for _ in 0..50 {
            let url = if cursor.is_empty() { format!("{path}{sep}limit=200") } else { format!("{path}{sep}limit=200&cursor={}", urlencode(&cursor)) };
            let page = self.get(&url).await?;
            if let Some(items) = page.get(field).and_then(Value::as_array) {
                out.extend(items.iter().cloned());
            }
            let more = page.pointer("/pageInfo/hasMore").and_then(Value::as_bool).unwrap_or(false);
            let next = page.pointer("/pageInfo/nextCursor").and_then(Value::as_str).unwrap_or("");
            if !more || next.is_empty() {
                break;
            }
            cursor = next.to_string();
        }
        Ok(out)
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&self.key).header("Accept", "application/json");
            for (k, v) in headers {
                req = req.header(*k, *v);
            }
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let retry_header = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).map(str::to_string);
            let text = resp.text().await?;
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                let v: Value = serde_json::from_str(&text).map_err(|e| Error::Provider(format!("boat answered {status} with a body that is not JSON: {e}")))?;
                // The envelope says ok:false only with an error status, but trust it either way.
                if v.get("ok").and_then(Value::as_bool) == Some(false) {
                    return Err(Refusal::parse(status.as_u16(), None, &text).into_error());
                }
                return Ok(v);
            }
            let refusal = Refusal::parse(status.as_u16(), retry_header.as_deref(), &text);
            if attempt < RETRIES && refusal.retryable() {
                attempt += 1;
                let wait = refusal.retry_after.unwrap_or(Duration::from_secs(1 << attempt)).min(MAX_WAIT);
                tokio::time::sleep(wait).await;
                continue;
            }
            if status == StatusCode::UNAUTHORIZED && refusal.message.is_empty() {
                return Err(Error::Auth("boat refused the API key".into()));
            }
            return Err(refusal.into_error());
        }
    }
}

/// How long one transfer of `size` bytes may take: two minutes, plus a
/// second per 512 KiB so a slow uplink is not cut off mid-file.
pub fn transfer_timeout(size: u64) -> Duration {
    Duration::from_secs(120 + size / (512 * 1024))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A path segment for an id the API handed out; anything unexpected is
/// percent-encoded so it cannot change the path.
pub fn seg(id: &str) -> String {
    urlencode(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_error_envelope() {
        let body = r#"{"ok":false,"type":"sandbox.error","status":409,"code":"sandbox_running_stop_first","message":"Stop it first.","error":{"code":"sandbox_running_stop_first","message":"Stop it first.","status":409},"requestId":"req_1"}"#;
        let r = Refusal::parse(409, None, body);
        assert_eq!((r.code.as_str(), r.message.as_str(), r.request_id.as_str()), ("sandbox_running_stop_first", "Stop it first.", "req_1"));
        assert!(!r.retryable());
        assert_eq!(r.into_error().to_string(), "boat: Stop it first. (sandbox_running_stop_first)");
    }

    #[test]
    fn nested_only_and_non_json_bodies_still_say_something() {
        let r = Refusal::parse(400, None, r#"{"error":{"code":"invalid_name","message":"Bad name"}}"#);
        assert_eq!((r.code.as_str(), r.message.as_str()), ("invalid_name", "Bad name"));
        let r = Refusal::parse(502, None, "Bad Gateway\n<html>");
        assert_eq!((r.code.as_str(), r.message.as_str()), ("", "Bad Gateway"));
    }

    #[test]
    fn waits_only_for_short_retry_hints() {
        let r = Refusal::parse(429, Some("3"), r#"{"code":"rate_limited","message":"minute window full"}"#);
        assert_eq!(r.retry_after, Some(Duration::from_secs(3)));
        assert!(r.retryable());
        assert!(matches!(r.into_error(), Error::RateLimited));
        let hour = Refusal::parse(429, Some("1800"), r#"{"code":"rate_limited"}"#);
        assert!(!hour.retryable());
        let busy = Refusal::parse(503, None, r#"{"code":"out_of_capacity","retryAfterSeconds":2}"#);
        assert_eq!(busy.retry_after, Some(Duration::from_secs(2)));
        assert!(busy.retryable());
        // A full sandbox count is a 429 too, but not a rate limit: it says what is full.
        let full = Refusal::parse(429, None, r#"{"code":"limit_reached","message":"100 sandboxes at once"}"#);
        assert_eq!(full.into_error().to_string(), "boat: 100 sandboxes at once (limit_reached)");
    }

    #[test]
    fn auth_errors_map_to_auth() {
        assert!(matches!(Refusal::parse(401, None, r#"{"code":"unauthorized","message":"bad key"}"#).into_error(), Error::Auth(_)));
        assert!(matches!(Refusal::parse(404, None, r#"{"code":"not_found","message":"no"}"#).into_error(), Error::NotFound(_)));
    }

    #[test]
    fn handed_out_paths_resolve_against_the_origin() {
        let api = Api::new("https://boat.dev/api/v1/", "boat_x".into()).unwrap();
        assert_eq!(api.absolute("https://files.boat.dev/u/abc?sig=1"), "https://files.boat.dev/u/abc?sig=1");
        assert_eq!(api.absolute("/api/v1/files/abc?sig=1"), "https://boat.dev/api/v1/files/abc?sig=1");
        assert_eq!(Api::new("http://127.0.0.1:9", "k".into()).unwrap().absolute("x"), "http://127.0.0.1:9/x");
    }

    #[test]
    fn big_transfers_get_more_time() {
        assert_eq!(transfer_timeout(0), Duration::from_secs(120));
        assert_eq!(transfer_timeout(1024 * 1024 * 1024), Duration::from_secs(120 + 2048));
    }

    #[test]
    fn ids_cannot_escape_their_path_segment() {
        assert_eq!(seg("bx_23456789"), "bx_23456789");
        assert_eq!(seg("../me"), "..%2Fme");
        assert_eq!(urlencode("a b/c"), "a%20b%2Fc");
    }
}
