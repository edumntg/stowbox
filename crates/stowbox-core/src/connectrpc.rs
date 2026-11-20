//! A small client for the Connect protocol (connectrpc.com) with protobuf
//! messages, for services that speak Connect instead of gRPC (E2B's envd).
//!
//! Unary calls are a plain POST of one message. A server stream is a POST of
//! one enveloped message whose answer is a run of envelopes: a flags byte, a
//! four-byte big-endian length, then that many bytes. The last envelope has
//! flag 0x02 and carries JSON: `{}` on success, `{"error":{code,message}}`
//! otherwise. Lengths come from the network, so each one is checked against
//! [`MAX_MESSAGE`] before a byte is kept, and the buffer never holds more
//! than one message plus one read.

use crate::http::read_capped;
use crate::{Error, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::Value;
use std::marker::PhantomData;
use std::time::Duration;

/// Largest message accepted from a server. envd sends output in chunks of
/// a few KiB; the cap only has to stop a hostile length from allocating.
pub const MAX_MESSAGE: usize = 4 << 20;
/// Flag on the envelope that ends a stream.
const END_STREAM: u8 = 0x02;
/// Flag for a compressed envelope; nothing here asks for compression.
const COMPRESSED: u8 = 0x01;
/// How long one unary call may take.
const UNARY_TIMEOUT: Duration = Duration::from_secs(30);

/// One envelope taken off the front of `buf`, or `None` while it is still
/// incomplete. The length is checked before anything is copied.
pub fn take_envelope(buf: &mut Vec<u8>) -> Result<Option<(u8, Vec<u8>)>> {
    let Some(&[flags, a, b, c, d]) = buf.get(..5) else { return Ok(None) };
    let len = u32::from_be_bytes([a, b, c, d]) as usize;
    if len > MAX_MESSAGE {
        return Err(Error::Provider(format!("the server sent a {len}-byte message; the limit is {MAX_MESSAGE}")));
    }
    if flags & COMPRESSED != 0 {
        return Err(Error::Provider("the server sent a compressed message, which was not asked for".into()));
    }
    let Some(body) = buf.get(5..5 + len) else { return Ok(None) };
    let body = body.to_vec();
    buf.drain(..5 + len);
    Ok(Some((flags, body)))
}

/// One message in an envelope with no flags.
pub fn envelope(message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + message.len());
    out.push(0);
    out.extend_from_slice(&(message.len() as u32).to_be_bytes());
    out.extend_from_slice(message);
    out
}

/// A Connect error (`{code, message}`) as a Stowbox error. `who` names the
/// service in the message.
pub fn error_from(who: &str, v: &Value) -> Error {
    let code = v.get("code").and_then(Value::as_str).unwrap_or("unknown");
    let message: String = v.get("message").and_then(Value::as_str).unwrap_or("").chars().take(300).collect();
    let message = if message.is_empty() { code.to_string() } else { message };
    match code {
        "unauthenticated" => Error::Auth(format!("{who}: {message}")),
        "permission_denied" => Error::Auth(format!("{who}: permission denied: {message}")),
        "not_found" => Error::NotFound(format!("{who}: {message}")),
        "resource_exhausted" => Error::RateLimited,
        "unavailable" | "deadline_exceeded" => Error::Network(format!("{who}: {message}")),
        _ => Error::Provider(format!("{who}: {message}")),
    }
}

/// The error in an end-of-stream envelope, if it carries one.
pub fn end_of_stream(who: &str, body: &[u8]) -> Result<()> {
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    match v.get("error") {
        Some(e) if !e.is_null() => Err(error_from(who, e)),
        _ => Ok(()),
    }
}

/// A Connect service at `base` (`https://host[:port]`), with headers sent on
/// every call. Header values are marked sensitive, so they never print.
pub struct Client {
    http: reqwest::Client,
    base: String,
    headers: HeaderMap,
    who: &'static str,
}

impl Client {
    pub fn new(http: reqwest::Client, base: &str, headers: &[(&'static str, String)], who: &'static str) -> Result<Self> {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            let mut v = HeaderValue::from_str(value).map_err(|_| Error::Provider(format!("{who}: a header value has characters a header cannot carry")))?;
            v.set_sensitive(true);
            map.insert(HeaderName::from_static(name), v);
        }
        map.insert("connect-protocol-version", HeaderValue::from_static("1"));
        Ok(Self { http, base: base.trim_end_matches('/').to_string(), headers: map, who })
    }

    /// `POST {base}/{method}` with one message, for one message back.
    /// `method` is `package.Service/Method`.
    pub async fn unary<Req: prost::Message, Resp: prost::Message + Default>(&self, method: &str, req: &Req) -> Result<Resp> {
        let resp = self
            .http
            .post(format!("{}/{method}", self.base))
            .headers(self.headers.clone())
            .header("content-type", "application/proto")
            .timeout(UNARY_TIMEOUT)
            .body(req.encode_to_vec())
            .send()
            .await?;
        let status = resp.status();
        if let Some(len) = resp.content_length().filter(|&l| l as usize > MAX_MESSAGE) {
            return Err(Error::Provider(format!("{}: answered with {len} bytes", self.who)));
        }
        let body = read_capped(resp, MAX_MESSAGE, "envd").await?;
        if !status.is_success() {
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            return Err(if v.is_object() { error_from(self.who, &v) } else { http_error(self.who, status.as_u16()) });
        }
        Resp::decode(body.as_slice()).map_err(|e| Error::Provider(format!("{}: an answer that is not the expected message: {e}", self.who)))
    }

    /// `POST {base}/{method}` with one message, for a stream of messages.
    /// `idle` is how long the stream may stay silent; servers that send
    /// keepalives (envd does, when asked) stay well inside it.
    pub async fn server_stream<Req: prost::Message, Resp: prost::Message + Default>(
        &self,
        method: &str,
        req: &Req,
        extra: &[(&'static str, String)],
        idle: Duration,
    ) -> Result<Stream<Resp>> {
        let mut builder = self
            .http
            .post(format!("{}/{method}", self.base))
            .headers(self.headers.clone())
            .header("content-type", "application/connect+proto")
            .body(envelope(&req.encode_to_vec()));
        for (k, v) in extra {
            builder = builder.header(*k, v.as_str());
        }
        let resp = tokio::time::timeout(idle, builder.send()).await.map_err(|_| Error::Network(format!("{}: no answer to the stream request", self.who)))??;
        let status = resp.status();
        if !status.is_success() {
            let body = read_capped(resp, 64 << 10, "envd").await.unwrap_or_default();
            let v: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
            return Err(if v.is_object() { error_from(self.who, &v) } else { http_error(self.who, status.as_u16()) });
        }
        Ok(Stream { resp, buf: Vec::new(), done: false, idle, who: self.who, _message: PhantomData })
    }
}

fn http_error(who: &str, status: u16) -> Error {
    match status {
        401 => Error::Auth(format!("{who} refused the credentials")),
        403 => Error::Auth(format!("{who} refused the call")),
        404 => Error::NotFound(format!("{who}: no such endpoint or box")),
        429 => Error::RateLimited,
        502..=504 => Error::Network(format!("{who} is not answering ({status})")),
        _ => Error::Provider(format!("{who} answered {status}")),
    }
}

/// The messages of a server stream, read as they arrive.
pub struct Stream<T> {
    resp: reqwest::Response,
    buf: Vec<u8>,
    done: bool,
    idle: Duration,
    who: &'static str,
    _message: PhantomData<fn() -> T>,
}

impl<T: prost::Message + Default> Stream<T> {
    /// The next message, `None` once the server ended the stream cleanly.
    pub async fn next(&mut self) -> Result<Option<T>> {
        loop {
            if self.done {
                return Ok(None);
            }
            if let Some((flags, body)) = take_envelope(&mut self.buf)? {
                if flags & END_STREAM != 0 {
                    self.done = true;
                    end_of_stream(self.who, &body)?;
                    return Ok(None);
                }
                return T::decode(body.as_slice()).map(Some).map_err(|e| Error::Provider(format!("{}: a stream message that does not parse: {e}", self.who)));
            }
            let chunk = tokio::time::timeout(self.idle, self.resp.chunk()).await.map_err(|_| Error::Network(format!("{} went quiet", self.who)))??;
            match chunk {
                Some(c) => self.buf.extend_from_slice(&c),
                None => {
                    self.done = true;
                    return Err(Error::Network(format!("{} ended the stream without saying it was done", self.who)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelopes_round_trip_and_wait_for_the_whole_message() {
        let mut buf = envelope(b"hello");
        buf.extend_from_slice(&envelope(b""));
        let half = buf.split_off(7);
        let mut partial = buf;
        assert_eq!(take_envelope(&mut partial).unwrap(), None);
        partial.extend_from_slice(&half);
        assert_eq!(take_envelope(&mut partial).unwrap(), Some((0, b"hello".to_vec())));
        assert_eq!(take_envelope(&mut partial).unwrap(), Some((0, Vec::new())));
        assert!(partial.is_empty());
        assert_eq!(take_envelope(&mut partial).unwrap(), None);
    }

    #[test]
    fn a_hostile_length_is_refused_before_any_allocation() {
        let mut buf = vec![0, 0xff, 0xff, 0xff, 0xff];
        assert!(take_envelope(&mut buf).is_err());
        let mut compressed = vec![COMPRESSED, 0, 0, 0, 1, b'x'];
        assert!(take_envelope(&mut compressed).is_err());
    }

    #[test]
    fn random_bytes_never_panic_the_decoder() {
        // A cheap stand-in for a fuzzer: xorshift bytes, some shaped like
        // envelopes, through the decoder until it stops taking any.
        let mut x: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..20_000 {
            let len = (next() % 64) as usize;
            let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            if next() % 2 == 0 && buf.len() >= 5 {
                buf[0] &= END_STREAM;
                let n = (buf.len() - 5) as u32 + (next() % 3) as u32;
                buf[1..5].copy_from_slice(&n.to_be_bytes());
            }
            while let Ok(Some(_)) = take_envelope(&mut buf) {}
        }
    }

    #[test]
    fn end_of_stream_errors_map_to_stowbox_errors() {
        assert!(end_of_stream("envd", b"{}").is_ok());
        assert!(end_of_stream("envd", b"").is_ok());
        assert!(matches!(end_of_stream("envd", br#"{"error":{"code":"unauthenticated","message":"bad token"}}"#), Err(Error::Auth(_))));
        assert!(matches!(end_of_stream("envd", br#"{"error":{"code":"not_found"}}"#), Err(Error::NotFound(_))));
        let long = format!(r#"{{"error":{{"code":"internal","message":"{}"}}}}"#, "x".repeat(5000));
        let Err(e) = end_of_stream("envd", long.as_bytes()) else { panic!("expected an error") };
        assert!(e.to_string().len() < 400);
    }
}
