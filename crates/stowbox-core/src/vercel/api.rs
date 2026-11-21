//! Vercel's REST API at `api.vercel.com`: a bearer token, the team in a
//! `teamId` query on every call, JSON bodies, errors as
//! `{ "error": { code, message } }`.

use crate::daytona::api::seg;
use crate::{http, Error, Result};
use reqwest::Method;
use serde_json::Value;
use std::time::Duration;

/// Largest answer read. A page of sandboxes is well under it.
const MAX_BODY: usize = 8 << 20;
/// How long a streaming answer may take to start.
const STREAM_START: Duration = Duration::from_secs(60);

pub struct Api {
    http: reqwest::Client,
    /// Streams last as long as a command, so this client has no overall
    /// timeout; the reader bounds each wait.
    stream_http: reqwest::Client,
    base: String,
    token: String,
    team: String,
}

/// `error.message`, then `error.code`, then the first line of the body.
pub fn error_message(body: &str) -> String {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = v.get("error").unwrap_or(&v);
    let pick = |k: &str| e.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(|s| s.chars().take(300).collect::<String>());
    pick("message").or_else(|| pick("code")).unwrap_or_else(|| body.lines().next().unwrap_or("").chars().take(200).collect())
}

pub fn into_error(status: u16, body: &str) -> Error {
    let m = error_message(body);
    match status {
        401 => Error::Auth(if m.is_empty() { "Vercel refused the token".into() } else { format!("Vercel refused the token: {m}") }),
        403 => Error::Auth(format!("refused: {m}")),
        404 => Error::NotFound(m),
        429 => Error::RateLimited,
        _ => Error::Provider(format!("vercel: {m}")),
    }
}

impl Api {
    pub fn new(base: &str, token: String, team: String) -> Result<Self> {
        let ua = concat!("stowbox/", env!("CARGO_PKG_VERSION"));
        let http = reqwest::Client::builder().user_agent(ua).connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(60)).build()?;
        // A command stream has no keepalives of its own, so TCP's find a
        // peer that went away without a reset.
        let stream_http = reqwest::Client::builder().user_agent(ua).connect_timeout(Duration::from_secs(10)).tcp_keepalive(Duration::from_secs(30)).build()?;
        Ok(Self { http, stream_http, base: base.trim_end_matches('/').to_string(), token, team })
    }

    fn url(&self, path: &str) -> String {
        let sep = if path.contains('?') { '&' } else { '?' };
        format!("{}{path}{sep}teamId={}", self.base, seg(&self.team))
    }

    pub async fn get(&self, path: &str) -> Result<Value> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::POST, path, Some(body)).await
    }

    pub async fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call(Method::PATCH, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value> {
        self.call(Method::DELETE, path, None).await
    }

    /// A POST whose answer streams; the caller reads it. Errors are read
    /// and mapped like any other call's.
    pub async fn post_stream(&self, path: &str, body: Value) -> Result<reqwest::Response> {
        let resp = tokio::time::timeout(STREAM_START, self.stream_http.post(self.url(path)).bearer_auth(&self.token).json(&body).send())
            .await
            .map_err(|_| Error::Network("Vercel did not start answering within a minute".into()))??;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = http::read_capped(resp, 64 << 10, "Vercel").await.unwrap_or_default();
        Err(into_error(status.as_u16(), &String::from_utf8_lossy(&text)))
    }

    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = self.url(path);
        let mut attempt = 0;
        loop {
            let mut req = self.http.request(method.clone(), &url).bearer_auth(&self.token).header("Accept", "application/json");
            if let Some(b) = &body {
                req = req.json(b);
            }
            let resp = req.send().await?;
            let status = resp.status();
            let retry_after = http::header(&resp);
            let bytes = http::read_capped(resp, MAX_BODY, "Vercel").await?;
            let text = String::from_utf8_lossy(&bytes);
            if status.is_success() {
                if text.trim().is_empty() {
                    return Ok(Value::Null);
                }
                return serde_json::from_str(&text).map_err(|e| Error::Provider(format!("Vercel answered {status} with a body that is not JSON: {e}")));
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
    fn reads_vercel_errors() {
        assert_eq!(error_message(r#"{"error":{"code":"not_found","message":"Sandbox not found"}}"#), "Sandbox not found");
        assert_eq!(error_message(r#"{"error":{"code":"forbidden"}}"#), "forbidden");
        assert_eq!(error_message("bad gateway\n<html>"), "bad gateway");
        assert!(matches!(into_error(401, "{}"), Error::Auth(_)));
        assert!(matches!(into_error(404, "{}"), Error::NotFound(_)));
        assert_eq!(into_error(400, r#"{"error":{"message":"timeout too long"}}"#).to_string(), "vercel: timeout too long");
    }

    #[test]
    fn the_team_goes_on_every_call_escaped() {
        let api = Api::new("https://api.vercel.com/", "t".into(), "team_a&b".into()).unwrap();
        assert_eq!(api.url("/v2/sandboxes"), "https://api.vercel.com/v2/sandboxes?teamId=team_a%26b");
        assert_eq!(api.url("/v2/sandboxes?project=p"), "https://api.vercel.com/v2/sandboxes?project=p&teamId=team_a%26b");
    }
}
