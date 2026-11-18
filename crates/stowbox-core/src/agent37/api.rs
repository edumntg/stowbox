//! Agent37's Sandbox API: bearer auth with a workspace `sk_live_` key, plain
//! JSON, lists wrapped in `{ data }`, errors as `{ error: { code, message } }`
//! and `503 try_again` while an instance is between states.

use crate::{Error, Result};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

const RETRIES: u32 = 3;

pub struct Api {
    http: reqwest::Client,
    base: String,
    /// `sandbox.agent37.app`, where exec is answered by the instance's own
    /// machine. It takes the key as `X-Agent37-Key`.
    sandbox: String,
    key: String,
}

/// `(code, message)` of an error body; the first line of whatever came
/// back when it is not the usual envelope.
pub fn error_parts(body: &str) -> (String, String) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = v.get("error").cloned().unwrap_or(Value::Null);
    let code = e.get("code").and_then(Value::as_str).unwrap_or("").to_string();
    let message = e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(200).collect());
    (code, message)
}

pub fn into_error(status: u16, body: &str) -> Error {
    let (code, m) = error_parts(body);
    match status {
        401 => Error::Auth(if m.is_empty() { "Agent37 refused the API key".into() } else { m }),
        403 => Error::Auth(format!("refused: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ if code.is_empty() => Error::Provider(format!("agent37: {m}")),
        _ => Error::Provider(format!("agent37: {m} ({code})")),
    }
}

impl Api {
    pub fn new(base: &str, sandbox: &str, key: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            // A create answers once the instance runs, which takes a while.
            .timeout(Duration::from_secs(180))
            .build()?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), sandbox: sandbox.trim_end_matches('/').to_string(), key })
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Option<Value>) -> Result<Value> {
        self.call(Method::POST, path, body).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }

    /// Runs `command` through the REST exec, which has no stdin or TTY but
    /// can be root. `background` answers at once with the pid; otherwise
    /// the answer carries `exit_code`, `stdout` and `stderr`.
    pub async fn exec(&self, id: &str, command: &str, root: bool, background: bool) -> Result<Value> {
        let url = format!("{}/instances/{}/exec", self.sandbox, seg(id));
        let mut body = serde_json::Map::new();
        body.insert("command".into(), command.into());
        if root {
            body.insert("user".into(), "root".into());
        }
        if background {
            body.insert("background".into(), true.into());
        }
        let resp = self.http.post(&url).header("X-Agent37-Key", &self.key).json(&body).send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(into_error(status.as_u16(), &text));
        }
        serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Agent37's exec answered with a body that is not JSON: {e}")))
    }

    /// The `data` of a list answer.
    pub async fn list(&self, path: &str) -> Result<Vec<Value>> {
        let v = self.get(path).await?;
        Ok(v.get("data").and_then(Value::as_array).cloned().unwrap_or_default())
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&self.key).header("Accept", "application/json");
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let text = resp.text().await?;
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Agent37 answered {status} with a body that is not JSON: {e}")));
            }
            // A create is not retried: a second one would make a second box.
            let retryable = matches!(status.as_u16(), 429 | 503) || (status.as_u16() == 409 && error_parts(&text).0 == "try_again");
            if attempt < RETRIES && retryable && !(method == Method::POST && path == "/instances") {
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1 << attempt)).await;
                continue;
            }
            return Err(into_error(status.as_u16(), &text));
        }
    }
}

/// An instance id as a path segment; anything unexpected is percent-encoded.
pub fn seg(s: &str) -> String {
    crate::daytona::api::seg(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_error_envelope() {
        let body = r#"{"error":{"code":"insufficient_balance","message":"Add balance to your workspace and try again."}}"#;
        assert_eq!(into_error(402, body).to_string(), "agent37: Add balance to your workspace and try again. (insufficient_balance)");
        assert!(matches!(into_error(401, r#"{"error":{"code":"unauthorized","message":"Invalid API key."}}"#), Error::Auth(_)));
        assert!(matches!(into_error(404, r#"{"error":{"code":"not_found","message":"Resource not found."}}"#), Error::NotFound(_)));
        assert_eq!(into_error(502, "Bad gateway\n<html>").to_string(), "agent37: Bad gateway");
    }
}
