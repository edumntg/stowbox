//! Newline-delimited JSON read off a streaming HTTP answer, one value per
//! line, for providers that stream command output that way (Vercel's
//! commands, Blaxel's processes).
//!
//! The lines come from the network: one longer than [`MAX_LINE`] is an
//! error before it is kept, and a stream that stays silent longer than its
//! idle limit, when it has one, is taken as dead.

use crate::{Error, Result};
use serde_json::Value;
use std::time::Duration;

/// Longest line accepted. Output arrives in chunks of a few KiB.
pub const MAX_LINE: usize = 4 << 20;

/// The values in a streaming answer, read as they arrive.
pub struct Lines {
    resp: reqwest::Response,
    buf: Vec<u8>,
    idle: Option<Duration>,
    who: &'static str,
    done: bool,
}

/// The first whole line in `buf`, taken off it, or `None` until one ends.
/// Blank lines are skipped.
pub fn take_line(buf: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        let nl = buf.iter().position(|&b| b == b'\n')?;
        let mut line: Vec<u8> = buf.drain(..=nl).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if !line.iter().all(u8::is_ascii_whitespace) {
            return Some(line);
        }
    }
}

fn parse(who: &str, line: &[u8]) -> Result<Value> {
    serde_json::from_slice(line).map_err(|e| Error::Provider(format!("{who} streamed a line that is not JSON: {e}")))
}

impl Lines {
    /// `idle` bounds each wait for more; `None` for a server that sends no
    /// keepalives, whose quiet commands would otherwise look dead.
    pub fn new(resp: reqwest::Response, idle: Option<Duration>, who: &'static str) -> Self {
        Self { resp, buf: Vec::new(), idle, who, done: false }
    }

    /// The next value, `None` once the answer has ended.
    pub async fn next(&mut self) -> Result<Option<Value>> {
        loop {
            if let Some(line) = take_line(&mut self.buf) {
                return parse(self.who, &line).map(Some);
            }
            if self.buf.len() > MAX_LINE {
                return Err(Error::Provider(format!("{} streamed a line over {MAX_LINE} bytes", self.who)));
            }
            if self.done {
                // A last line with no newline after it.
                let rest = std::mem::take(&mut self.buf);
                if rest.iter().all(u8::is_ascii_whitespace) {
                    return Ok(None);
                }
                return parse(self.who, &rest).map(Some);
            }
            let chunk = match self.idle {
                Some(idle) => tokio::time::timeout(idle, self.resp.chunk()).await.map_err(|_| Error::Network(format!("{} went quiet", self.who)))??,
                None => self.resp.chunk().await?,
            };
            match chunk {
                Some(c) => self.buf.extend_from_slice(&c),
                None => self.done = true,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_come_off_whole_and_blank_ones_are_skipped() {
        let mut buf = b"{\"a\":1}\r\n\n  \n{\"b\"".to_vec();
        assert_eq!(take_line(&mut buf), Some(b"{\"a\":1}".to_vec()));
        assert_eq!(take_line(&mut buf), None);
        assert_eq!(buf, b"{\"b\"");
        buf.extend_from_slice(b":2}\n");
        assert_eq!(take_line(&mut buf), Some(b"{\"b\":2}".to_vec()));
        assert!(buf.is_empty());
    }

    #[test]
    fn a_line_that_is_not_json_is_an_error() {
        assert!(parse("x", b"{\"a\":").is_err());
        assert_eq!(parse("x", b"{\"a\":1}").unwrap()["a"], 1);
    }
}
