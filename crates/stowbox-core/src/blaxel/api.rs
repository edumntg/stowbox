//! Blaxel's REST APIs: the control plane at `api.blaxel.ai/v0` and each
//! sandbox's own API at its URL. Both take the workspace's API key as a
//! bearer token and the workspace in `X-Blaxel-Workspace`; errors are
//! `{ error, message }`.

use crate::{http, Error, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

/// The API version Stowbox was written against. Lists come back paginated
/// from 2026-04-28 on.
const VERSION: &str = "2026-09-22";
/// Largest answer read. A page of sandboxes is well under it.
const MAX_BODY: usize = 8 << 20;
/// How long a streaming answer may take to start.
const STREAM_START: Duration = Duration::from_secs(60);

/// A request body.
pub enum Body {
    None,
    Json(Value),
    Bytes(Vec<u8>),
}

pub struct Api {
    http: reqwest::Client,
    /// Log streams last as long as a command, so this client has no
    /// overall timeout; the reader bounds each wait.
    stream_http: reqwest::Client,
    base: String,
    headers: HeaderMap,
}

/// `message`, then `error`, then the body, each to its first line: Blaxel
/// appends a server stack trace to some messages.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let pick =
        |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| s.lines().next()).filter(|s| !s.is_empty()).map(|s| s.chars().take(300).collect::<String>());
    pick("message").or_else(|| pick("error")).unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(200).collect())
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 => Error::Auth(if m.is_empty() { "Blaxel refused the API key".into() } else { format!("Blaxel refused the API key: {m}") }),
        403 => Error::Auth(format!("refused: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ => Error::Provider(format!("blaxel: {m}")),
    }
}

impl Api {
    pub fn new(base: &str, key: &str, workspace: &str) -> Result<Self> {
        let ua = concat!("stowbox/", env!("CARGO_PKG_VERSION"));
        let bad = || Error::Auth("the Blaxel API key or workspace has characters a header cannot carry".into());
        let mut headers = HeaderMap::new();
        let mut auth = HeaderValue::from_str(&format!("Bearer {key}")).map_err(|_| bad())?;
        auth.set_sensitive(true);
        headers.insert("authorization", auth);
        headers.insert("x-blaxel-workspace", HeaderValue::from_str(workspace).map_err(|_| bad())?);
        headers.insert("blaxel-version", HeaderValue::from_static(VERSION));
        let http = reqwest::Client::builder().user_agent(ua).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(130)).build()?;
        // TCP keepalives find a stream whose peer went away without a reset.
        let stream_http = reqwest::Client::builder().user_agent(ua).connect_timeout(Duration::from_secs(10)).tcp_keepalive(Duration::from_secs(30)).build()?;
        Ok(Self { http, stream_http, base: base.trim_end_matches('/').to_string(), headers })
    }

    /// The headers every call carries, for a WebSocket upgrade.
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers.iter().filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))).collect()
    }

    /// A control-plane path as a URL.
    pub fn control(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// A call to `url`, a control-plane URL or a checked sandbox URL.
    pub async fn call(&self, method: Method, url: &str, body: Body) -> Result<Value> {
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), url).headers(self.headers.clone()).header("Accept", "application/json");
            req = match &body {
                Body::None => req,
                Body::Json(v) => req.json(v),
                Body::Bytes(b) => req.header("content-type", "application/octet-stream").body(b.clone()),
            };
            let resp = req.send().await?;
            let status = resp.status();
            let retry_after = http::header(&resp);
            let bytes = http::read_capped(resp, MAX_BODY, "Blaxel").await?;
            let text = String::from_utf8_lossy(&bytes);
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Blaxel answered {status} with a body that is not JSON: {e}")));
            }
            if let Some(wait) = http::wait(attempt, &method, status.as_u16(), retry_after) {
                attempt += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(into_error(status.as_u16(), &text));
        }
    }

    pub async fn get(&self, url: &str) -> Result<Value> {
        self.call(Method::GET, url, Body::None).await
    }

    pub async fn post(&self, url: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, url, Body::Json(body)).await
    }

    pub async fn delete(&self, url: &str) -> Result<Value> {
        self.call(Method::DELETE, url, Body::None).await
    }

    /// A GET whose NDJSON answer streams; the caller reads it.
    pub async fn get_stream(&self, url: &str) -> Result<reqwest::Response> {
        let req = self.stream_http.get(url).headers(self.headers.clone()).header("Accept", "application/x-ndjson");
        let resp =
            tokio::time::timeout(STREAM_START, req.send()).await.map_err(|_| Error::Network("Blaxel did not start answering within a minute".into()))??;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = http::read_capped(resp, 64 << 10, "Blaxel").await.unwrap_or_default();
        Err(into_error(status.as_u16(), &String::from_utf8_lossy(&text)))
    }
}

/// The whole body, refusing one longer than `cap`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_blaxel_errors() {
        assert_eq!(error_message(r#"{"code":404,"error":"not_found","message":"Sandbox x not found"}"#), "Sandbox x not found");
        assert_eq!(error_message(r#"{"error":"forbidden"}"#), "forbidden");
        assert_eq!(error_message("{\"message\":\"not enabled\\n\\tat manager.go:42\"}"), "not enabled");
        assert_eq!(error_message("bad gateway\n<html>"), "bad gateway");
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(404, "{}"), Error::NotFound(_)));
    }

    #[test]
    fn the_key_never_prints_and_a_bad_header_is_refused() {
        let api = Api::new("https://api.blaxel.ai/v0/", "bl_secret", "ws").unwrap();
        assert!(!format!("{:?}", api.headers).contains("bl_secret"));
        assert_eq!(api.control("/sandboxes"), "https://api.blaxel.ai/v0/sandboxes");
        assert!(Api::new("https://x", "bl_a\r\nEvil: 1", "ws").is_err());
    }
}
