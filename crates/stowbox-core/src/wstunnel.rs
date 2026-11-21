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
    over(tls(host, port).await?, host, headers).await
}

/// A TLS connection to `host:port`, verified against the web PKI roots, for
/// an HTTP/1.1 upgrade.
pub(crate) async fn tls(host: &str, port: u16) -> Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
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
    tokio::time::timeout(Duration::from_secs(15), tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp))
        .await
        .map_err(|_| Error::Network(format!("no TLS answer from {host} within 15 s")))?
        .map_err(|e| Error::Network(format!("tls to {host}: {e}")))
}

/// The handshake and the pump over any stream, so tests need no TLS.
pub async fn over<S>(mut stream: S, host: &str, headers: &[(String, String)]) -> Result<DuplexStream>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let rest = handshake(&mut stream, host, "/", headers).await?;
    let (ours, theirs) = tokio::io::duplex(256 * 1024);
    tokio::spawn(pump(stream, theirs, rest));
    Ok(ours)
}

/// Sends the upgrade for `path` (with its query) and checks the answer.
/// Returns what the server sent after the answer's head: frames may ride in
/// with it.
pub(crate) async fn handshake<S>(stream: &mut S, host: &str, path: &str, headers: &[(String, String)]) -> Result<Vec<u8>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // The host and path come from the provider's API and the headers carry
    // the key the person typed: a line break or a space in any of them would
    // add to the request line or headers.
    if host.contains(['\r', '\n'])
        || !path.starts_with('/')
        || path.bytes().any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        || headers.iter().any(|(k, v)| k.contains(['\r', '\n', ':']) || v.contains(['\r', '\n']))
    {
        return Err(Error::Provider("a WebSocket header holds a line break; check the API key for stray characters".into()));
    }
    let mut nonce = [0u8; 16];
    getrandom::getrandom(&mut nonce).map_err(|e| Error::Network(format!("no randomness for the WebSocket key: {e}")))?;
    let key = base64::engine::general_purpose::STANDARD.encode(nonce);
    let mut req = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 2048];
    let (head, rest) = loop {
        if let Some(split) = split_head(&buf) {
            break split;
        }
        if buf.len() > MAX_HEAD {
            return Err(Error::Network(format!("{host} sent no end to its WebSocket answer")));
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            return Err(Error::Network(format!("{host} closed the connection during the WebSocket handshake")));
        }
        buf.extend_from_slice(chunk.get(..n).unwrap_or_default());
    };
    check_answer(&String::from_utf8_lossy(head), &key, host)?;
    Ok(rest.to_vec())
}

/// The handshake answer's head and what came after its blank line, once
/// `buf` holds the whole head.
fn split_head(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    let at = buf.windows(4).position(|w| w == b"\r\n\r\n")?;
    let (head, rest) = buf.split_at_checked(at)?;
    Some((head, rest.strip_prefix(b"\r\n\r\n")?))
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
    let accept =
        lines.find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("sec-websocket-accept")).map(|(_, v)| v.trim().to_string()));
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
    out.extend(payload.iter().zip(mask.iter().cycle()).map(|(b, m)| b ^ m));
    out
}

/// A frame read off the front of `buf`: its opcode, its payload and the
/// bytes it took. `None` while the frame is incomplete. The bytes come from
/// the network, so every field is read with a bounds check and the length
/// is capped before anything is copied.
pub fn decode(buf: &[u8]) -> Result<Option<(u8, Vec<u8>, usize)>> {
    Ok(decode_frame(buf)?.map(|(_, op, payload, used)| (op, payload, used)))
}

/// One decoded frame: FIN, opcode, payload, and the bytes it took.
pub type Frame = (bool, u8, Vec<u8>, usize);

/// [`decode`], with whether the frame is the last of its message (FIN).
pub fn decode_frame(buf: &[u8]) -> Result<Option<Frame>> {
    let [b0, b1, rest @ ..] = buf else {
        return Ok(None);
    };
    let fin = b0 & 0x80 != 0;
    let opcode = b0 & 0x0f;
    let masked = b1 & 0x80 != 0;
    let (len, rest) = match b1 & 0x7f {
        126 => match rest.split_first_chunk::<2>() {
            Some((n, rest)) => (u64::from(u16::from_be_bytes(*n)), rest),
            None => return Ok(None),
        },
        127 => match rest.split_first_chunk::<8>() {
            Some((n, rest)) => (u64::from_be_bytes(*n), rest),
            None => return Ok(None),
        },
        n => (u64::from(n), rest),
    };
    if len > MAX_FRAME {
        return Err(Error::Network(format!("the tunnel sent a {len}-byte frame")));
    }
    let (mask, rest) = if masked {
        match rest.split_first_chunk::<4>() {
            Some((m, rest)) => (Some(*m), rest),
            None => return Ok(None),
        }
    } else {
        (None, rest)
    };
    let len = usize::try_from(len).map_err(|_| Error::Network(format!("the tunnel sent a {len}-byte frame")))?;
    let Some(payload) = rest.get(..len) else {
        return Ok(None);
    };
    let mut payload = payload.to_vec();
    if let Some(m) = mask {
        payload.iter_mut().zip(m.iter().cycle()).for_each(|(b, k)| *b ^= k);
    }
    // `rest` is a suffix of `buf`, so the header is what lies before it.
    let used = buf.len() - rest.len() + len;
    Ok(Some((fin, opcode, payload, used)))
}

pub(crate) fn new_mask() -> [u8; 4] {
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
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            match decode(&buf) {
                Ok(Some((op, payload, used))) => {
                    buf.drain(..used);
                    match op {
                        OP_CLOSE => return,
                        OP_PING => {
                            let _ = pong_tx.send(payload).await;
                        }
                        0x0..=0x2 if !payload.is_empty() && local_w.write_all(&payload).await.is_err() => return,
                        _ => {}
                    }
                    continue;
                }
                Ok(None) => {}
                Err(_) => return,
            }
            match sock_r.read(&mut chunk).await {
                Ok(0) | Err(_) => return,
                Ok(n) => buf.extend_from_slice(chunk.get(..n).unwrap_or_default()),
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
                        if sock_w.write_all(&encode(OP_BINARY, chunk.get(..n).unwrap_or_default(), new_mask())).await.is_err() {
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
        let mut f = vec![0x82, 127];
        f.extend_from_slice(&u64::MAX.to_be_bytes());
        assert!(decode(&f).is_err());
    }

    #[test]
    fn a_header_cut_short_is_incomplete_in_every_length_form() {
        for f in [&[0x82u8][..], &[0x82, 126], &[0x82, 126, 0], &[0x82, 127, 0, 0, 0], &[0x82, 0x85], &[0x82, 0x85, 1, 2, 3], &[0x82, 0xfe, 0, 10, 1]] {
            assert!(decode(f).unwrap().is_none(), "{f:?}");
        }
    }

    /// Whatever the server sends, decoding answers and never panics, and a
    /// frame it accepts never claims more bytes than it was given.
    #[test]
    fn hostile_bytes_never_panic() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..20_000 {
            let len = (next() % 40) as usize;
            let mut buf: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Bias the length byte towards the extended forms.
            if let Some(b) = buf.get_mut(1) {
                *b = [*b, 0x7e, 0x7f, 0xfe, 0xff][(next() % 5) as usize];
            }
            if let Ok(Some((_, payload, used))) = decode(&buf) {
                assert!(used <= buf.len() && payload.len() <= used, "{buf:?}");
            }
        }
    }

    #[test]
    fn the_handshake_head_ends_at_the_first_blank_line() {
        assert_eq!(split_head(b"HTTP/1.1 101\r\nA: b\r\n\r\n\x82\x00"), Some((&b"HTTP/1.1 101\r\nA: b"[..], &b"\x82\x00"[..])));
        assert_eq!(split_head(b"HTTP/1.1 101\r\n\r"), None);
        assert_eq!(split_head(b""), None);
    }

    #[tokio::test]
    async fn line_breaks_in_headers_are_refused_before_anything_is_sent() {
        let (client, _server) = tokio::io::duplex(1024);
        let e = over(client, "h", &[("X-Agent37-Key".into(), "sk_live_x\r\nEvil: 1".into())]).await.unwrap_err();
        assert!(e.to_string().contains("line break"), "{e}");
        let (client, _server) = tokio::io::duplex(1024);
        assert!(over(client, "h\r\nEvil: 1", &[]).await.is_err());
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
            let mut answer =
                format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", accept_for(&key))
                    .into_bytes();
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
