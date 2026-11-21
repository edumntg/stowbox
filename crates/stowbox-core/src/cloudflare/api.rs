//! The bridge's REST API: JSON under `/v1`, the bridge token as a bearer
//! token, errors as `{ error }`. The bridge is a Worker the person deploys
//! (`bridge/cloudflare` in this repository), so its URL is theirs: it must
//! be a plain `https://` origin, since the token goes to it.

use crate::{http, Error, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

/// Largest answer read. A list of the most boxes a bridge keeps is far
/// under it.
const MAX_BODY: usize = 1 << 20;

pub struct Api {
    http: reqwest::Client,
    /// `https://host[:port]`, without a trailing slash.
    base: String,
    headers: HeaderMap,
}

/// The origin of a bridge URL as the person typed it: `https://`, a DNS
/// name or IP, an optional port, and nothing after but one `/`.
pub fn origin(url: &str) -> Result<String> {
    let bad = || Error::Auth("the bridge URL must be https:// and a host, such as https://stowbox-bridge.example.workers.dev".into());
    let rest = url.trim().strip_prefix("https://").ok_or_else(bad)?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p.parse::<u16>().ok().filter(|&p| p > 0).ok_or_else(bad)?)),
        None => (authority, None),
    };
    if !crate::websocket::valid_host(host) {
        return Err(bad());
    }
    let host = host.to_ascii_lowercase();
    Ok(match port {
        Some(p) => format!("https://{host}:{p}"),
        None => format!("https://{host}"),
    })
}

/// The bridge's `error`, to its first line and 300 characters.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    match v.get("error").and_then(Value::as_str) {
        Some(e) => e.lines().next().unwrap_or("").chars().take(300).collect(),
        None => body.lines().next().unwrap_or("").chars().take(200).collect(),
    }
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 | 403 => Error::Auth(format!("the bridge refused the token: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        // A bridge deployed without its STOWBOX_TOKEN secret says so; any
        // other 503 is Cloudflare's edge.
        503 if m.contains("STOWBOX_TOKEN") => Error::Auth(format!("the bridge is not set up: {m}")),
        503 => Error::Network(format!("Cloudflare could not reach the bridge just now: {m}")),
        _ => Error::Provider(format!("cloudflare bridge: {m}")),
    }
}

impl Api {
    pub fn new(url: &str, token: &str) -> Result<Self> {
        let base = origin(url)?;
        let mut auth =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| Error::Auth("the bridge token has characters a header cannot carry".into()))?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("authorization", auth);
        // Creating a box starts its container, which can take a while cold.
        let http = reqwest::Client::builder()
            .user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { http, base, headers })
    }

    /// The exec socket's URL for `path`.
    pub fn ws_url(&self, path: &str) -> String {
        format!("wss://{}{path}", self.base.trim_start_matches("https://"))
    }

    /// The token, for the exec socket's upgrade.
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers.iter().filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))).collect()
    }

    pub async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), &url).headers(self.headers.clone()).header("Accept", "application/json");
            if let Some(b) = body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let retry_after = http::header(&resp);
            let bytes = http::read_capped(resp, MAX_BODY, "the Cloudflare bridge").await?;
            let text = String::from_utf8_lossy(&bytes);
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| Error::Provider(format!("the bridge answered {status} with a body that is not JSON: {e}")));
            }
            if let Some(wait) = http::wait(attempt, &method, status.as_u16(), retry_after) {
                attempt += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(into_error(status.as_u16(), &text));
        }
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_origin_is_a_bridge() {
        assert_eq!(origin("https://stowbox-bridge.me.workers.dev/").unwrap(), "https://stowbox-bridge.me.workers.dev");
        assert_eq!(origin(" https://Bridge.Example.com:8443 ").unwrap(), "https://bridge.example.com:8443");
        for bad in [
            "http://a.com",
            "a.com",
            "https://",
            "https://a.com/v1",
            "https://a.com?x=1",
            "https://user@a.com",
            "https://a.com:0",
            "https://a.com:99999",
            "https://-a.com",
            "https://a.com#x",
            "https://a.com\r\nX: 1",
        ] {
            assert!(origin(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn errors_name_the_reason_and_the_token_never_prints() {
        assert_eq!(error_message(r#"{"error":"no such box"}"#), "no such box");
        assert_eq!(error_message("bad gateway\n<html>"), "bad gateway");
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(503, r#"{"error":"the bridge has no STOWBOX_TOKEN secret of at least 32 characters"}"#), Error::Auth(_)));
        assert!(matches!(into_error(503, "Service Unavailable"), Error::Network(_)));
        assert!(matches!(into_error(404, "{}"), Error::NotFound(_)));
        let api = Api::new("https://b.example.com", "tok-secret").unwrap();
        assert!(!format!("{:?}", api.headers).contains("tok-secret"));
        assert_eq!(api.ws_url("/v1/boxes/x/exec"), "wss://b.example.com/v1/boxes/x/exec");
        assert!(Api::new("https://b.example.com", "a\r\nEvil: 1").is_err());
    }
}
