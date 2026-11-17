//! A byte stream carried over a WebSocket, for providers whose boxes take
//! SSH only through their edge (Agent37: `wss://{id}-22022.agent37.app`
//! with the API key in a header). The SSH client gets a plain duplex
//! stream; a task moves bytes between it and the socket as binary frames.
//!
//! Only what such a tunnel needs is here: the client handshake with extra
//! headers, masked binary frames out, any data frame in (SSH is a byte
//! stream, so fragments need no reassembly), pings answered, and a close
//! ending the stream.

use crate::{Error, Result};
use base64::Engine;
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Largest handshake answer read before giving up.
const MAX_HEAD: usize = 16 * 1024;
/// Largest frame accepted from the server; SSH packets are far smaller.
const MAX_FRAME: u64 = 4 * 1024 * 1024;
/// What one outgoing frame carries at most.
const CHUNK: usize = 32 * 1024;

const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;
const OP_BINARY: u8 = 0x2;

/// Opens `wss://{host}:{port}/` with `headers` on the upgrade and returns
/// the stream inside it.
pub async fn connect(host: &str, port: u16, headers: &[(String, String)]) -> Result<DuplexStream> {
    let tcp = tokio::time::timeout(Duration::from_secs(15), tokio::net::TcpStream::connect((host, port)))
        .await
        .map_err(|_| Error::Network(format!("no answer from {host}:{port} within 15 s")))??;
    let _ = tcp.set_nodelay(true);
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let mut config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::Network(format!("tls: {e}")))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = rustls::pki_types::ServerName::try_from(host.to_string()).map_err(|e| Error::Network(format!("tls: {host}: {e}")))?;
    let tls = tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.map_err(|e| Error::Network(format!("tls to {host}: {e}")))?;
    over(tls, host, headers).await
}

/// The handshake and the pump over any stream, so tests need no TLS.
pub async fn over<S>(mut stream: S, host: &str, headers: &[(String, String)]) -> Result<DuplexStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|e| Error::Network(format!("no randomness for the WebSocket key: {e}")))?;
    let key = base64::engine::general_purpose::STANDARD.encode(nonce);
    let mut req = format!("GET / HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(1024);
    let end = loop {
        if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buf.len() > MAX_HEAD {
            return Err(Error::Network(format!("{host} sent no end to its WebSocket answer")));
        }
        let mut chunk = [0u8; 2048];
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(Error::Network(format!("{host} closed the connection during the WebSocket handshake")));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    check_answer(&head, &key, host)?;
    let rest = buf[end + 4..].to_vec();

    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    tokio::spawn(pump(stream, theirs, rest));
    Ok(ours)
}

/// The value the server must answer a `Sec-WebSocket-Key` with.
pub fn accept_for(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

fn check_answer(head: &str, key: &str, host: &str) -> Result<()> {
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or("");
    let code = status.split_whitespace().nth(1).unwrap_or("");
    match code {
        "101" => {}
        "401" | "403" => return Err(Error::Auth(format!("{host} refused the tunnel ({status}); the API key may be wrong or revoked"))),
        "404" => return Err(Error::NotFound(format!("{host} has no box behind it ({status})"))),
        _ => return Err(Error::Network(format!("{host} refused the tunnel: {}", if status.is_empty() { "no answer" } else { status }))),
    }
    let accept = lines.find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept")).map(|(_, v)| v.trim().to_string()));
    if accept.as_deref() != Some(accept_for(key).as_str()) {
        return Err(Error::Network(format!("{host} answered the WebSocket upgrade with the wrong accept key")));
    }
    Ok(())
}

/// One client frame: FIN, `opcode`, masked with `mask`.
pub fn encode(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let len = payload.len();
    let mut out = Vec::with_capacity(len + 14);
    out.push(0x80 | opcode);
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= 0xffff {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// A frame read off the front of `buf`: its opcode, its payload and the
/// bytes it took. `None` while the frame is incomplete.
pub fn decode(buf: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>> {
    if buf.len() < 2 {
        return Ok(None);
    }
    let opcode = buf[0] & 0x0f;
    let masked = buf[1] & 0x80 != 0;
    let mut at = 2;
    let len = match buf[1] & 0x7f {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            at = 4;
            u16::from_be_bytes([buf[2], buf[3]]) as u64
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            at = 10;
            u64::from_be_bytes(buf[2..10].try_into().unwrap())
        }
        n => n as u64,
    };
    if len > MAX_FRAME {
        return Err(Error::Network(format!("the tunnel sent a {len}-byte frame")));
    }
    let mask = if masked {
        if buf.len() < at + 4 {
            return Ok(None);
        }
        at += 4;
        Some([buf[at - 4], buf[at - 3], buf[at - 2], buf[at - 1]])
    } else {
        None
    };
    let len = len as usize;
    if buf.len() < at + len {
        return Ok(None);
    }
    let mut payload = buf[at..at + len].to_vec();
    if let Some(m) = mask {
        payload.iter_mut().enumerate().for_each(|(i, b)| *b ^= m[i % 4]);
    }
    Ok(Some((opcode, payload, at + len)))
}

fn new_mask() -> [u8; 4] {
    let mut m = [0u8; 4];
    let _ = getrandom::getrandom(&mut m);
    m
}

/// Moves bytes both ways until either side ends; `rest` is what came after
/// the handshake answer.
async fn pump<S>(socket: S, local: DuplexStream, rest: Vec<u8>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sock_r, mut sock_w) = tokio::io::split(socket);
    let (mut local_r, mut local_w) = tokio::io::split(local);
    // Pongs go out through the writer, which owns the socket's write half.
    let (pong_tx, mut pong_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);

    let inbound = async move {
        let mut buf = rest;
        loop {
            while let Ok(Some((op, payload, used))) = decode(&buf) {
                buf.drain(..used);
                match op {
                    OP_CLOSE => return,
                    OP_PING => {
                        let _ = pong_tx.send(payload).await;
                    }
                    0x0..=0x2 if !payload.is_empty() => {
                        if local_w.write_all(&payload).await.is_err() {
                            return;
                        }
                    }
                    _ => {}
                }
            }
            if decode(&buf).is_err() {
                return;
            }
            let mut chunk = vec![0u8; 64 * 1024];
            match sock_r.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    };

    let outbound = async move {
        let mut chunk = vec![0u8; CHUNK];
        loop {
            tokio::select! {
                n = local_r.read(&mut chunk) => match n {
                    Ok(0) | Err(_) => {
                        let _ = sock_w.write_all(&encode(OP_CLOSE, &[], new_mask())).await;
                        let _ = sock_w.shutdown().await;
                        return;
                    }
                    Ok(n) => {
                        if sock_w.write_all(&encode(OP_BINARY, &chunk[..n], new_mask())).await.is_err() {
                            return;
                        }
                    }
                },
                Some(p) = pong_rx.recv() => {
                    if sock_w.write_all(&encode(OP_PONG, &p, new_mask())).await.is_err() {
                        return;
                    }
                }
            }
        }
    };

    // Either side ending ends the tunnel; the duplex halves drop with it.
    tokio::select! {
        _ = inbound => {}
        _ = outbound => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_accept_key_is_the_rfc_example() {
        assert_eq!(accept_for("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn frames_round_trip_at_every_length_form() {
        for len in [0usize, 1, 125, 126, 65_535, 65_536, 200_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
            let f = encode(OP_BINARY, &payload, [1, 2, 3, 4]);
            let (op, got, used) = decode(&f).unwrap().unwrap();
            assert_eq!((op, used), (OP_BINARY, f.len()), "len {len}");
            assert_eq!(got, payload, "len {len}");
            // Every prefix short of the whole frame is incomplete, not wrong.
            for cut in [1, f.len() / 2, f.len() - 1] {
                assert!(decode(&f[..cut.min(f.len() - 1)]).unwrap().is_none(), "len {len} cut {cut}");
            }
        }
    }

    #[test]
    fn a_huge_frame_is_refused() {
        let mut f = vec![0x82, 127];
        f.extend_from_slice(&(MAX_FRAME + 1).to_be_bytes());
        assert!(decode(&f).is_err());
    }

    #[test]
    fn refusals_say_what_went_wrong() {
        let e = check_answer("HTTP/1.1 401 Unauthorized\r\nContent-Length: 0", "k", "x-22022.agent37.app").unwrap_err();
        assert!(matches!(e, Error::Auth(_)), "{e}");
        let e = check_answer("HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: nope", "k", "h").unwrap_err();
        assert!(e.to_string().contains("accept key"), "{e}");
        assert!(check_answer(&format!("HTTP/1.1 101 Switching Protocols\r\nsec-websocket-accept: {}", accept_for("k")), "k", "h").is_ok());
    }

    /// A server on the other end of an in-memory pipe: answers the upgrade
    /// with a frame already behind it, pings, echoes one frame, then closes.
    #[tokio::test]
    async fn bytes_cross_both_ways_and_pings_are_answered() {
        let (client, mut server) = tokio::io::duplex(1 << 20);
        let srv = tokio::spawn(async move {
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                let mut b = [0u8; 1];
                server.read_exact(&mut b).await.unwrap();
                head.push(b[0]);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.contains("X-Agent37-Key: sk_live_test\r\n"), "{head}");
            let key = head.lines().find_map(|l| l.strip_prefix("Sec-WebSocket-Key: ")).unwrap().trim().to_string();
            let mut answer = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_for(&key)).into_bytes();
            // Server frames are unmasked; one rides in with the answer.
            answer.extend_from_slice(&[0x82, 6]);
            answer.extend_from_slice(b"SSH-2.");
            answer.extend_from_slice(&[0x89, 2, b'h', b'i']);
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
            server.write_all(&[0x88, 0]).await.unwrap();
            seen
        });

        let mut s = over(client, "ab12cd34ef-22022.agent37.app", &[("X-Agent37-Key".into(), "sk_live_test".into())]).await.unwrap();
        let mut got = [0u8; 6];
        s.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"SSH-2.");
        s.write_all(b"hello").await.unwrap();
        let seen = srv.await.unwrap();
        assert!(seen.contains(&(OP_PONG, b"hi".to_vec())), "{seen:?}");
        assert!(seen.contains(&(OP_BINARY, b"hello".to_vec())), "{seen:?}");
        // The server's close ends the stream.
        let mut rest = Vec::new();
        s.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }
}
