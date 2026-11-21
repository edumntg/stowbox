//! WebSocket messages, for providers whose terminal is a WebSocket of text
//! and binary messages (Vercel's interactive sessions, Blaxel's terminal).
//!
//! The upgrade, TLS and frame codec are [`crate::wstunnel`]'s; this adds
//! what a message protocol needs on top: a path and query on the upgrade,
//! text kept apart from binary, fragments joined up to [`MAX_MESSAGE`], and
//! the close code. One task owns the socket: it answers pings, and sends a
//! close frame once every [`Socket::send`] sender is dropped.

use crate::wstunnel::{self, decode_frame, encode, new_mask};
use crate::{Error, Result};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

/// Largest message accepted from a server, fragments joined.
pub const MAX_MESSAGE: usize = 4 << 20;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

const OP_CONTINUATION: u8 = 0x0;
const OP_TEXT: u8 = 0x1;
const OP_BINARY: u8 = 0x2;
const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// A message to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
}

/// What the server sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Text(String),
    Binary(Vec<u8>),
    /// The server closed the socket, with its close code if it gave one.
    /// Nothing follows.
    Closed {
        code: Option<u16>,
        reason: String,
    },
}

/// An open socket. Dropping `send` closes it; dropping `recv` ends the task.
pub struct Socket {
    pub send: mpsc::Sender<Message>,
    pub recv: mpsc::Receiver<Result<Event>>,
}

/// A host from a provider or the person, with only the characters a DNS
/// name or an IPv4 address has, so it cannot change a request line.
pub fn valid_host(host: &str) -> bool {
    (1..=253).contains(&host.len()) && !host.starts_with(['.', '-']) && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `wss://host[:port]/path?query` as host, port and the request target.
/// The URL comes from a provider's API, so the host may hold only the
/// characters a DNS name has.
pub fn parse_url(url: &str) -> Result<(String, u16, String)> {
    let bad = || Error::Provider("the provider gave a terminal URL that is not a plain wss:// URL".into());
    let rest = url.strip_prefix("wss://").ok_or_else(bad)?;
    let (authority, target) = match rest.find(['/', '?']) {
        Some(at) => rest.split_at_checked(at).ok_or_else(bad)?,
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| bad())?),
        None => (authority, 443),
    };
    if !valid_host(host) || port == 0 {
        return Err(bad());
    }
    let target = if target.starts_with('?') {
        format!("/{target}")
    } else if target.is_empty() {
        "/".to_string()
    } else {
        target.to_string()
    };
    Ok((host.to_ascii_lowercase(), port, target))
}

/// Opens `url` with `headers` on the upgrade.
pub async fn connect(url: &str, headers: &[(String, String)]) -> Result<Socket> {
    let (host, port, target) = parse_url(url)?;
    let tls = wstunnel::tls(&host, port).await?;
    over(tls, &host, &target, headers).await
}

/// The upgrade and the socket task over any stream, so tests need no TLS.
pub async fn over<S>(mut stream: S, host: &str, target: &str, headers: &[(String, String)]) -> Result<Socket>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let rest = tokio::time::timeout(HANDSHAKE_TIMEOUT, wstunnel::handshake(&mut stream, host, target, headers))
        .await
        .map_err(|_| Error::Network(format!("{host} did not finish the WebSocket handshake within 15 s")))??;
    let (send, out_rx) = mpsc::channel(32);
    let (in_tx, recv) = mpsc::channel(64);
    tokio::spawn(run(stream, rest, out_rx, in_tx));
    Ok(Socket { send, recv })
}

/// A message being joined from fragments.
struct Partial {
    opcode: u8,
    data: Vec<u8>,
}

/// What one incoming frame means for the reader.
enum Step {
    Deliver(Event),
    Reply(Vec<u8>),
    Nothing,
    Close(Event, Vec<u8>),
}

/// Turns one frame into what to do about it, joining fragments into
/// `partial`. Frames come from the network, so a fragment out of order or a
/// message past [`MAX_MESSAGE`] is an error, not a panic or an allocation.
fn step(partial: &mut Option<Partial>, fin: bool, opcode: u8, payload: Vec<u8>) -> Result<Step> {
    let message = |opcode: u8, data: Vec<u8>| {
        if opcode == OP_TEXT {
            Event::Text(String::from_utf8_lossy(&data).into_owned())
        } else {
            Event::Binary(data)
        }
    };
    match opcode {
        OP_TEXT | OP_BINARY => {
            if partial.is_some() {
                return Err(Error::Network("the server started a message inside another".into()));
            }
            if fin {
                return Ok(Step::Deliver(message(opcode, payload)));
            }
            *partial = Some(Partial { opcode, data: payload });
            Ok(Step::Nothing)
        }
        OP_CONTINUATION => {
            let Some(p) = partial.as_mut() else {
                return Err(Error::Network("the server continued a message it never started".into()));
            };
            if p.data.len() + payload.len() > MAX_MESSAGE {
                return Err(Error::Network(format!("the server sent a message over {MAX_MESSAGE} bytes")));
            }
            p.data.extend_from_slice(&payload);
            match partial.take() {
                Some(p) if fin => Ok(Step::Deliver(message(p.opcode, p.data))),
                kept => {
                    *partial = kept;
                    Ok(Step::Nothing)
                }
            }
        }
        OP_CLOSE => {
            let (code, reason) = match payload.split_first_chunk::<2>() {
                Some((c, rest)) => (Some(u16::from_be_bytes(*c)), String::from_utf8_lossy(rest).chars().take(200).collect()),
                None => (None, String::new()),
            };
            // Echo the code back, as the closing handshake asks.
            let echo = encode(OP_CLOSE, payload.get(..2).unwrap_or_default(), new_mask());
            Ok(Step::Close(Event::Closed { code, reason }, echo))
        }
        OP_PING => Ok(Step::Reply(encode(OP_PONG, &payload, new_mask()))),
        OP_PONG => Ok(Step::Nothing),
        other => Err(Error::Network(format!("the server sent a frame of unknown type {other:#x}"))),
    }
}

async fn run<S>(mut sock: S, mut buf: Vec<u8>, mut out_rx: mpsc::Receiver<Message>, in_tx: mpsc::Sender<Result<Event>>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut partial = None;
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        // Everything already buffered first: frames can ride in with the
        // handshake answer.
        loop {
            let frame = match decode_frame(&buf) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    let _ = in_tx.send(Err(e)).await;
                    return;
                }
            };
            let (fin, opcode, payload, used) = frame;
            buf.drain(..used.min(buf.len()));
            match step(&mut partial, fin, opcode, payload) {
                Ok(Step::Deliver(ev)) => {
                    if in_tx.send(Ok(ev)).await.is_err() {
                        return;
                    }
                }
                Ok(Step::Reply(frame)) => {
                    if sock.write_all(&frame).await.is_err() {
                        let _ = in_tx.send(Err(Error::Network("the WebSocket connection broke".into()))).await;
                        return;
                    }
                }
                Ok(Step::Nothing) => {}
                Ok(Step::Close(ev, echo)) => {
                    let _ = sock.write_all(&echo).await;
                    let _ = sock.shutdown().await;
                    let _ = in_tx.send(Ok(ev)).await;
                    return;
                }
                Err(e) => {
                    let _ = in_tx.send(Err(e)).await;
                    return;
                }
            }
        }
        tokio::select! {
            n = sock.read(&mut chunk) => match n {
                Ok(0) => {
                    let _ = in_tx.send(Ok(Event::Closed { code: None, reason: "the connection ended".into() })).await;
                    return;
                }
                Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or_default()),
                Err(e) => {
                    let _ = in_tx.send(Err(Error::Network(format!("the WebSocket connection broke: {e}")))).await;
                    return;
                }
            },
            m = out_rx.recv() => {
                let frame = match m {
                    Some(Message::Text(t)) => encode(OP_TEXT, t.as_bytes(), new_mask()),
                    Some(Message::Binary(b)) => encode(OP_BINARY, &b, new_mask()),
                    None => {
                        // 1000: a normal close.
                        let _ = sock.write_all(&encode(OP_CLOSE, &1000u16.to_be_bytes(), new_mask())).await;
                        let _ = sock.shutdown().await;
                        return;
                    }
                };
                if sock.write_all(&frame).await.is_err() {
                    let _ = in_tx.send(Err(Error::Network("the WebSocket connection broke".into()))).await;
                    return;
                }
            }
            _ = in_tx.closed() => {
                let _ = sock.write_all(&encode(OP_CLOSE, &1000u16.to_be_bytes(), new_mask())).await;
                let _ = sock.shutdown().await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wstunnel::{accept_for, decode};

    #[test]
    fn urls_split_into_host_port_and_target() {
        assert_eq!(parse_url("wss://a.example.com/ws?t=1").unwrap(), ("a.example.com".into(), 443, "/ws?t=1".into()));
        assert_eq!(parse_url("wss://a.example.com:8443").unwrap(), ("a.example.com".into(), 8443, "/".into()));
        assert_eq!(parse_url("wss://a.example.com?x=y").unwrap(), ("a.example.com".into(), 443, "/?x=y".into()));
        for bad in [
            "ws://a.com/",
            "https://a.com",
            "wss://",
            "wss://user@a.com/",
            "wss://a.com:0/",
            "wss://a.com:99999/",
            "wss://a com/",
            "wss://-a.com/",
            "wss://a.com\r\n/",
        ] {
            assert!(parse_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn fragments_join_and_out_of_order_ones_are_refused() {
        let mut p = None;
        assert!(matches!(step(&mut p, false, OP_TEXT, b"he".to_vec()).unwrap(), Step::Nothing));
        assert!(matches!(step(&mut p, false, OP_PING, b"x".to_vec()).unwrap(), Step::Reply(_)));
        assert!(matches!(step(&mut p, false, OP_CONTINUATION, b"l".to_vec()).unwrap(), Step::Nothing));
        let Step::Deliver(ev) = step(&mut p, true, OP_CONTINUATION, b"lo".to_vec()).unwrap() else { panic!("not delivered") };
        assert_eq!(ev, Event::Text("hello".into()));
        assert!(step(&mut p, true, OP_CONTINUATION, b"x".to_vec()).is_err());
        assert!(matches!(step(&mut p, false, OP_BINARY, vec![1]).unwrap(), Step::Nothing));
        assert!(step(&mut p, true, OP_TEXT, vec![]).is_err());
        assert!(step(&mut None, true, 0x3, vec![]).is_err());
    }

    #[test]
    fn a_message_past_the_cap_is_refused() {
        let mut p = None;
        step(&mut p, false, OP_BINARY, vec![0; MAX_MESSAGE]).unwrap();
        assert!(step(&mut p, true, OP_CONTINUATION, vec![0]).is_err());
    }

    #[test]
    fn a_close_frame_gives_its_code_and_reason() {
        let mut payload = 1000u16.to_be_bytes().to_vec();
        payload.extend_from_slice(b"Shell exited");
        let Step::Close(ev, _) = step(&mut None, true, OP_CLOSE, payload).unwrap() else { panic!("not a close") };
        assert_eq!(ev, Event::Closed { code: Some(1000), reason: "Shell exited".into() });
        let Step::Close(ev, _) = step(&mut None, true, OP_CLOSE, vec![]).unwrap() else { panic!("not a close") };
        assert_eq!(ev, Event::Closed { code: None, reason: String::new() });
    }

    /// A server on an in-memory pipe: checks the request target, sends a
    /// fragmented text message and a ping, reads one message, then closes.
    #[tokio::test]
    async fn messages_cross_both_ways() {
        let (client, mut server) = tokio::io::duplex(1 << 20);
        let srv = tokio::spawn(async move {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0u8; 1];
                server.read_exact(&mut b).await.unwrap();
                head.push(b[0]);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("GET /terminal/ws?sessionId=a HTTP/1.1\r\n"), "{head}");
            let key = head.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap().trim().to_string();
            let mut answer = format!("HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_for(&key)).into_bytes();
            answer.extend_from_slice(&[0x01, 3]);
            answer.extend_from_slice(b"{\"a");
            answer.extend_from_slice(&[0x89, 1, b'p']);
            answer.extend_from_slice(&[0x80, 2]);
            answer.extend_from_slice(b"\"}");
            server.write_all(&answer).await.unwrap();
            let mut buf = Vec::new();
            let mut seen = Vec::new();
            while seen.len() < 2 {
                let mut chunk = [0u8; 4096];
                let n = server.read(&mut chunk).await.unwrap();
                buf.extend_from_slice(&chunk[..n]);
                while let Some((op, payload, used)) = decode(&buf).unwrap() {
                    buf.drain(..used);
                    seen.push((op, payload));
                }
            }
            let mut close = vec![0x88, 5];
            close.extend_from_slice(&4000u16.to_be_bytes());
            close.extend_from_slice(b"bye");
            server.write_all(&close).await.unwrap();
            seen
        });
        let mut s = over(client, "h", "/terminal/ws?sessionId=a", &[]).await.unwrap();
        assert_eq!(s.recv.recv().await.unwrap().unwrap(), Event::Text("{\"a\"}".into()));
        s.send.send(Message::Binary(b"ls\n".to_vec())).await.unwrap();
        let seen = srv.await.unwrap();
        assert!(seen.contains(&(OP_PONG, b"p".to_vec())), "{seen:?}");
        assert!(seen.contains(&(OP_BINARY, b"ls\n".to_vec())), "{seen:?}");
        assert_eq!(s.recv.recv().await.unwrap().unwrap(), Event::Closed { code: Some(4000), reason: "bye".into() });
        assert!(s.recv.recv().await.is_none());
    }

    #[tokio::test]
    async fn dropping_the_sender_closes_the_socket() {
        let (client, mut server) = tokio::io::duplex(1 << 16);
        let srv = tokio::spawn(async move {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0u8; 1];
                server.read_exact(&mut b).await.unwrap();
                head.push(b[0]);
            }
            let head = String::from_utf8(head).unwrap();
            let key = head.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap().trim().to_string();
            server.write_all(format!("HTTP/1.1 101 OK\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_for(&key)).as_bytes()).await.unwrap();
            let mut rest = Vec::new();
            server.read_to_end(&mut rest).await.unwrap();
            decode(&rest).unwrap().unwrap()
        });
        let s = over(client, "h", "/", &[]).await.unwrap();
        drop(s.send);
        let (op, payload, _) = srv.await.unwrap();
        assert_eq!((op, payload), (OP_CLOSE, 1000u16.to_be_bytes().to_vec()));
        drop(s.recv);
    }
}
