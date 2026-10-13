//! API key → JWT exchange. The JWT lives about an hour; we refresh it five
//! minutes early and read its real expiry from the `exp` claim when present.

use crate::{Error, Result};
use base64::Engine;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
const ASSUMED_LIFETIME: Duration = Duration::from_secs(55 * 60);

pub struct TokenExchange {
    http: reqwest::Client,
    api_key: String,
    auth_url: String,
    cached: Mutex<Option<Cached>>,
}

struct Cached {
    token: String,
    expires_at: SystemTime,
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
    #[serde(default)]
    expires_at: Option<serde_json::Value>,
}

impl TokenExchange {
    pub fn new(api_key: String, auth_url: String) -> Self {
        Self {
            http: reqwest::Client::builder()
                .user_agent(concat!("stowbox/", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("reqwest client"),
            api_key,
            auth_url,
            cached: Mutex::new(None),
        }
    }

    pub async fn token(&self) -> Result<String> {
        let mut guard = self.cached.lock().await;
        if let Some(c) = guard.as_ref() {
            if SystemTime::now() + REFRESH_MARGIN < c.expires_at {
                return Ok(c.token.clone());
            }
        }
        let fresh = self.exchange().await?;
        let token = fresh.token.clone();
        *guard = Some(fresh);
        Ok(token)
    }

    async fn exchange(&self) -> Result<Cached> {
        let resp = self
            .http
            .post(&self.auth_url)
            .json(&serde_json::json!({ "api_key": self.api_key }))
            .send()
            .await?;
        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(Error::Auth("boxd rejected the API key".into()));
        }
        if status.as_u16() == 429 {
            return Err(Error::RateLimited);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Auth(format!("token exchange failed ({status}): {}", body.trim())));
        }
        let body: TokenResponse = resp.json().await?;
        let expires_at = jwt_exp(&body.token)
            .or_else(|| body.expires_at.as_ref().and_then(parse_expires))
            .unwrap_or_else(|| SystemTime::now() + ASSUMED_LIFETIME);
        Ok(Cached { token: body.token, expires_at })
    }
}

/// `expires_at` may come as unix seconds, unix millis, or an RFC 3339 string.
/// Strings are ignored here because the JWT `exp` claim already covers them.
fn parse_expires(v: &serde_json::Value) -> Option<SystemTime> {
    let n = v.as_i64()?;
    let secs = if n > 10_000_000_000 { n / 1000 } else { n };
    Some(UNIX_EPOCH + Duration::from_secs(secs.max(0) as u64))
}

/// Reads the `exp` claim from a JWT without verifying it. We only use it to
/// decide when to refresh; the server does the verification.
pub fn jwt_exp(token: &str) -> Option<SystemTime> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()
        .or_else(|| base64::engine::general_purpose::URL_SAFE.decode(payload).ok())?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let exp = claims.get("exp")?.as_i64()?;
    Some(UNIX_EPOCH + Duration::from_secs(exp.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_exp_claim() {
        // header.payload.signature with payload {"exp":1700000000}
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"exp":1700000000}"#);
        let token = format!("eyJhbGciOiJIUzI1NiJ9.{payload}.sig");
        let exp = jwt_exp(&token).unwrap();
        assert_eq!(exp.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_700_000_000);
    }

    #[test]
    fn garbage_is_none() {
        assert!(jwt_exp("not-a-jwt").is_none());
        assert!(jwt_exp("a.b.c").is_none());
    }

    #[test]
    fn expires_accepts_seconds_and_millis() {
        let s = parse_expires(&serde_json::json!(1_700_000_000)).unwrap();
        let ms = parse_expires(&serde_json::json!(1_700_000_000_000i64)).unwrap();
        assert_eq!(s, ms);
        assert!(parse_expires(&serde_json::json!("2026-01-01T00:00:00Z")).is_none());
    }
}
