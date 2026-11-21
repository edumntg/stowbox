//! Daytona's REST API: bearer auth with an organization's API key, plain
//! JSON bodies, NestJS-style errors (`{ statusCode, message, error }`, where
//! `message` can be a list) and `Retry-After` on 429.

use crate::{http, Error, Result};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

pub struct Api {
    http: reqwest::Client,
    base: String,
    key: String,
}

/// The text of an error body: `message` (joined when it is a list), then
/// `error`, then the first line of whatever came back.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = match v.get("message") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; "),
        _ => String::new(),
    };
    let message = if message.is_empty() { v.get("error").and_then(Value::as_str).unwrap_or("").to_string() } else { message };
    if message.is_empty() {
        body.lines().next().unwrap_or("").chars().take(200).collect()
    } else {
        message
    }
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 => Error::Auth(if m.is_empty() { "Daytona refused the API key".into() } else { m }),
        403 => Error::Auth(format!("refused: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ => Error::Provider(format!("daytona: {m}")),
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
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Option<Value>) -> Result<Value> {
        self.call(Method::POST, path, body).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }

    /// Every item of a cursor-paged list (`{ items, nextCursor }`); a plain
    /// array is taken as the whole list.
    pub async fn list_all(&self, path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut cursor = String::new();
        let sep = if path.contains('?') { '&' } else { '?' };
        // A guard against a server that keeps handing out cursors.
        for _ in 0..50 {
            let url = if cursor.is_empty() { format!("{path}{sep}limit=100") } else { format!("{path}{sep}limit=100&cursor={}", seg(&cursor)) };
            let page = self.get(&url).await?;
            if let Value::Array(items) = page {
                out.extend(items);
                break;
            }
            if let Some(items) = page.get("items").and_then(Value::as_array) {
                out.extend(items.iter().cloned());
            }
            match page.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                Some(next) => cursor = next.to_string(),
                None => break,
            }
        }
        Ok(out)
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
            let retry_after = http::header(&resp);
            let text = resp.text().await?;
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Daytona answered {status} with a body that is not JSON: {e}")));
            }
            if let Some(wait) = http::wait(attempt, &method, status.as_u16(), retry_after) {
                attempt += 1;
                tokio::time::sleep(wait).await;
                continue;
            }
            return Err(into_error(status.as_u16(), &text));
        }
    }
}

/// A path segment or query value; anything unexpected is percent-encoded.
pub fn seg(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nest_errors_with_one_or_many_messages() {
        assert_eq!(error_message(r#"{"statusCode":404,"message":"Sandbox with ID x not found","error":"Not Found"}"#), "Sandbox with ID x not found");
        assert_eq!(
            error_message(r#"{"statusCode":400,"message":["cpu must be at most 4","memory must be an integer"],"error":"Bad Request"}"#),
            "cpu must be at most 4; memory must be an integer"
        );
        assert_eq!(error_message(r#"{"statusCode":500,"error":"Internal Server Error"}"#), "Internal Server Error");
        assert_eq!(error_message("upstream timed out\n<html>"), "upstream timed out");
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(404, r#"{"message":"gone"}"#), Error::NotFound(_)));
        assert_eq!(into_error(400, r#"{"message":"bad"}"#).to_string(), "daytona: bad");
    }

    #[test]
    fn ids_cannot_escape_their_segment() {
        assert_eq!(seg("4f1c-a2"), "4f1c-a2");
        assert_eq!(seg("../users/me"), "..%2Fusers%2Fme");
    }
}
