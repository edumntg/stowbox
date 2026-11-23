//! The Sprites REST API at `api.sprites.dev/v1`: JSON, the token as a
//! bearer token. Errors come back as plain text or as `{ error }` JSON.

use crate::{http, Error, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_API: &str = "https://api.sprites.dev";
/// Largest answer read. A page of 100 sprites is far under it.
const MAX_BODY: usize = 1 << 20;

pub struct Api {
    http: reqwest::Client,
    /// `https://host`, without a trailing slash.
    base: String,
    headers: HeaderMap,
}

/// The error's text, to its first line and 300 characters: `error` or
/// `message` from JSON, or the body itself.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let text = ["error", "message"].iter().find_map(|k| v.get(*k).and_then(Value::as_str)).unwrap_or(body);
    text.lines().next().unwrap_or("").chars().take(300).collect()
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 | 403 => Error::Auth(format!("Sprites refused the token: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ => Error::Provider(format!("sprites: {m}")),
    }
}

impl Api {
    pub fn new(base: &str, token: &str) -> Result<Self> {
        let mut auth = HeaderValue::from_str(&format!("Bearer {}", token.trim()))
            .map_err(|_| Error::Auth("the Sprites token has characters a header cannot carry".into()))?;
        auth.set_sensitive(true);
        let mut headers = HeaderMap::new();
        headers.insert("authorization", auth);
        // Creating a sprite is given two minutes, as Sprites' own SDK does.
        let http = reqwest::Client::builder()
            .user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), headers })
    }

    /// The exec socket's URL for `path_and_query`.
    pub fn ws_url(&self, path_and_query: &str) -> String {
        format!("{}{path_and_query}", self.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1))
    }

    /// The token, for the exec socket's upgrade.
    pub fn header_pairs(&self) -> Vec<(String, String)> {
        self.headers.iter().filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string()))).collect()
    }

    pub async fn call(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let text = self.send(method, path, body).await?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Sprites answered with a body that is not JSON: {e}")))
    }

    /// A POST whose answer is not used: the kill call streams NDJSON
    /// progress, and only its success matters.
    pub async fn post_ignoring_body(&self, path: &str) -> Result<()> {
        self.send(Method::POST, path, Some(&Value::Object(Default::default()))).await.map(drop)
    }

    /// Sends a request, with retries, and returns a successful answer's body.
    async fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<String> {
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
            let bytes = http::read_capped(resp, MAX_BODY, "Sprites").await?;
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if status.is_success() {
                return Ok(text);
            }
            if let Some(wait) = http::wait(attempt, &method, status.as_u16(), retry_after) {
                attempt += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(into_error(status.as_u16(), &text));
        }
    }

    /// A PUT with `data` as the body, sent once: a retry would copy the
    /// bytes again. Given as long as an upload over exec.
    pub async fn put_bytes(&self, path: &str, data: Vec<u8>) -> Result<Value> {
        let resp = self
            .http
            .put(format!("{}{path}", self.base))
            .headers(self.headers.clone())
            .header("Content-Type", "application/octet-stream")
            .timeout(Duration::from_secs(300))
            .body(data)
            .send()
            .await?;
        let status = resp.status();
        let text = String::from_utf8_lossy(&http::read_capped(resp, MAX_BODY, "Sprites").await?).into_owned();
        if !status.is_success() {
            return Err(into_error(status.as_u16(), &text));
        }
        serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Sprites answered with a body that is not JSON: {e}")))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_name_the_reason_and_the_token_never_prints() {
        assert_eq!(error_message(r#"{"error":"sprite not found"}"#), "sprite not found");
        assert_eq!(error_message(r#"{"message":"slow down","code":"sprite_creation_rate_limited"}"#), "slow down");
        assert_eq!(error_message("Missing or invalid authentication\nmore"), "Missing or invalid authentication");
        assert_eq!(error_message(&"x".repeat(1000)).len(), 300);
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(404, "Sprite not found"), Error::NotFound(m) if m == "Sprite not found"));
        assert!(matches!(into_error(429, ""), Error::RateLimited));
        assert!(matches!(into_error(500, "boom"), Error::Provider(_)));
        let api = Api::new(DEFAULT_API, "org/123/tok-secret").unwrap();
        assert!(!format!("{:?}", api.headers).contains("tok-secret"));
        assert_eq!(api.ws_url("/v1/sprites/a/exec?x=1"), "wss://api.sprites.dev/v1/sprites/a/exec?x=1");
        assert!(Api::new(DEFAULT_API, "a\r\nEvil: 1").is_err());
    }
}
