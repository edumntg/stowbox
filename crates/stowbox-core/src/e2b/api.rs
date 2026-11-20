//! E2B's REST API at `api.e2b.app`: the team's API key in `X-API-Key`, JSON
//! bodies, errors as `{ code, message }`.

use crate::{http, Error, Result};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

/// Largest answer read. A list of a few hundred sandboxes is well under it.
const MAX_BODY: usize = 8 << 20;

pub struct Api {
    http: reqwest::Client,
    base: String,
    key: String,
}

/// `message`, then `error_code`, then the first line of the body.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let pick = |k: &str| v.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    pick("message").or_else(|| pick("error_code")).unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(200).collect())
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 => Error::Auth(if m.is_empty() { "E2B refused the API key".into() } else { m }),
        403 => Error::Auth(format!("refused: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ => Error::Provider(format!("e2b: {m}")),
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

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }

    /// Every sandbox in `path`, following `X-Next-Token`.
    pub async fn list_all(&self, path: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut token = String::new();
        let sep = if path.contains('?') { '&' } else { '?' };
        for _ in 0..50 {
            let url =
                if token.is_empty() { format!("{path}{sep}limit=100") } else { format!("{path}{sep}limit=100&nextToken={}", crate::daytona::api::seg(&token)) };
            let (page, next) = self.call_with_next(Method::GET, &url, None).await?;
            if let Value::Array(items) = page {
                out.extend(items);
            }
            match next {
                Some(t) if !t.is_empty() => token = t,
                _ => break,
            }
        }
        Ok(out)
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        Ok(self.call_with_next(method, path, body).await?.0)
    }

    async fn call_with_next(&self, method: Method, path: &str, body: Option<Value>) -> Result<(Value, Option<String>)> {
        let url = format!("{}{}", self.base, path);
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), &url).header("X-API-Key", &self.key).header("Accept", "application/json");
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let retry_after = http::header(&resp);
            let next = resp.headers().get("x-next-token").and_then(|v| v.to_str().ok()).map(str::to_string);
            let bytes = http::read_capped(resp, MAX_BODY, "E2B").await?;
            let text = String::from_utf8_lossy(&bytes);
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok((Value::Null, next));
                }
                let v = serde_json::from_str(&text).map_err(|e| Error::Provider(format!("E2B answered {status} with a body that is not JSON: {e}")))?;
                return Ok((v, next));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_e2b_errors() {
        assert_eq!(error_message(r#"{"code":404,"message":"sandbox \"x\" not found"}"#), "sandbox \"x\" not found");
        assert_eq!(error_message(r#"{"code":503,"error_code":"sandbox_capacity_unavailable","message":""}"#), "sandbox_capacity_unavailable");
        assert_eq!(error_message("bad gateway\n<html>"), "bad gateway");
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(404, "{}"), Error::NotFound(_)));
        assert_eq!(into_error(400, r#"{"message":"timeout too long"}"#).to_string(), "e2b: timeout too long");
    }
}
