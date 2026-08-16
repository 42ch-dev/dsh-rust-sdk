//! Line-framed JSON-RPC 2.0 transport over async read/write halves.
//!
//! Each frame is one compact JSON document on its own line, terminated by
//! `\n` — the same framing as the reference clients for the DeepSeek Harness
//! runtime stdio protocol ([deepseek-harness](https://github.com/deepseek-ai/deepseek-harness)).
//!
//! The reader mirrors the reference transports' tolerance: blank lines and
//! malformed peer lines (non-JSON or invalid UTF-8) are skipped and logged via
//! `tracing`, never rejected, so a single garbage line cannot kill the stream.
//! The only local failure is the >16 MiB framing guard ([`MAX_LINE_LEN`]),
//! which bounds memory use and is *not* protocol behavior.

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::error::Error;

/// Upper bound for one incoming line, in bytes. A local framing guard against
/// unbounded memory growth (not protocol behavior): a longer line fails the
/// read with [`Error::SdkProtocol`].
const MAX_LINE_LEN: usize = 16 * 1024 * 1024; // 16 MiB

/// A line-framed JSON-RPC 2.0 transport over one read half and one write half.
///
/// [`JsonRpcLineTransport::write_frame`] serializes a JSON value compactly,
/// appends `\n`, and flushes; [`JsonRpcLineTransport::read_frame`] returns the
/// next JSON value from the peer, skipping blank lines and malformed lines
/// (reference parity: both reference clients ignore malformed peer lines and
/// keep reading). EOF on the read half yields [`None`].
pub struct JsonRpcLineTransport<R, W> {
    reader: BufReader<R>,
    writer: W,
    /// Scratch buffer reused across reads to avoid reallocating per frame.
    read_buf: Vec<u8>,
}

/// Serialize `value` compactly, append `\n`, and flush it to `writer`.
///
/// Shared by [`JsonRpcLineTransport::write_frame`] and the client's write
/// path (the `HarnessClient` keeps its stdin write half separate from the
/// read loop's stdout reader, so both serialize through this one helper).
pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &Value,
) -> Result<(), Error> {
    let line = serde_json::to_vec(value)?;
    writer.write_all(&line).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

impl<R, W> JsonRpcLineTransport<R, W>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Create a transport reading from `reader` and writing to `writer`.
    ///
    /// For the runtime stdio protocol, `reader` is the runtime's stdout and
    /// `writer` its stdin.
    pub fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            read_buf: Vec::new(),
        }
    }

    /// Serialize `value` compactly, append `\n`, and flush.
    pub async fn write_frame(&mut self, value: &Value) -> Result<(), Error> {
        write_frame(&mut self.writer, value).await
    }

    /// Read the next frame as a JSON value.
    ///
    /// Returns [`Ok(None)`] on EOF. Blank lines and lines that are not valid
    /// JSON (including invalid UTF-8) are skipped and logged, matching both
    /// reference clients; a line longer than [`MAX_LINE_LEN`] fails with
    /// [`Error::SdkProtocol`] as a local framing guard.
    pub async fn read_frame(&mut self) -> Result<Option<Value>, Error> {
        loop {
            self.read_buf.clear();
            if !self.read_line_into_buf().await? {
                return Ok(None);
            }
            let line = self.read_buf.trim_ascii();
            if line.is_empty() {
                continue; // blank line
            }
            match serde_json::from_slice(line) {
                Ok(value) => return Ok(Some(value)),
                Err(err) => {
                    tracing::debug!(
                        line_len = line.len(),
                        error = %err,
                        "ignoring malformed JSON-RPC line from peer \
                         (reference parity: skip malformed lines and keep reading)"
                    );
                    continue;
                }
            }
        }
    }

    /// Read one line (without its trailing `\n`) into `self.read_buf`.
    ///
    /// Returns `Ok(true)` when a line was read, `Ok(false)` on EOF. A partial
    /// line at EOF (no trailing `\n`) is still reported as a line — parity
    /// with `readline()` — and is skipped by the parse step if malformed.
    async fn read_line_into_buf(&mut self) -> Result<bool, Error> {
        loop {
            let buf = self.reader.fill_buf().await?;
            if buf.is_empty() {
                return Ok(!self.read_buf.is_empty());
            }
            let newline = buf.iter().position(|&b| b == b'\n');
            let content = newline.unwrap_or(buf.len());
            if self.read_buf.len() + content > MAX_LINE_LEN {
                // `content` is the whole chunk when the line has no newline
                // in it, so `observed` is a lower bound on the true length;
                // when the newline is in this chunk it is exact.
                let observed = self.read_buf.len() + content;
                return Err(Error::SdkProtocol {
                    message: format!(
                        "incoming line exceeds {MAX_LINE_LEN} bytes (16 MiB framing guard), \
                         observed {observed} bytes so far"
                    ),
                });
            }
            self.read_buf.extend_from_slice(&buf[..content]);
            let consumed = newline.map_or(buf.len(), |pos| pos + 1);
            self.reader.consume(consumed);
            if newline.is_some() {
                return Ok(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};

    /// Two `duplex(64)` pipes modeling the runtime's stdio pair: the
    /// transport's read half is fed by `server_tx`, and the transport's write
    /// half drains into `server_rx`.
    fn transport_pair() -> (
        JsonRpcLineTransport<DuplexStream, DuplexStream>,
        DuplexStream,
        DuplexStream,
    ) {
        let (client_rx, server_tx) = duplex(64);
        let (client_tx, server_rx) = duplex(64);
        (
            JsonRpcLineTransport::new(client_rx, client_tx),
            server_tx,
            server_rx,
        )
    }

    #[tokio::test]
    async fn write_then_read_frame_through_duplex() {
        let (mut transport, mut server_tx, mut server_rx) = transport_pair();

        // Client → server: the frame must be visible before the transport is
        // dropped, i.e. `write_frame` flushes.
        transport
            .write_frame(&json!({"id": "r1", "result": {"ok": true}}))
            .await
            .unwrap();
        let mut buf = vec![0u8; 1024];
        let n = server_rx.read(&mut buf).await.unwrap();
        assert_eq!(
            &buf[..n],
            b"{\"id\":\"r1\",\"result\":{\"ok\":true}}\n",
            "write_frame must emit one compact JSON line and flush"
        );

        // Server → client: a frame fed on the other pipe reads back as a Value.
        server_tx
            .write_all(b"{\"method\":\"session.status\",\"params\":{}}\n")
            .await
            .unwrap();
        let frame = transport.read_frame().await.unwrap().expect("one frame");
        assert_eq!(frame, json!({"method": "session.status", "params": {}}));
    }

    #[tokio::test]
    async fn multi_frame_sequence_reads_in_order() {
        let (mut transport, mut server_tx, _server_rx) = transport_pair();

        // A peer streaming frames: the writer can outpace the 64-byte pipe, so
        // it runs concurrently and is drained by the transport's read loop.
        let writer = tokio::spawn(async move {
            for i in 0..3 {
                let line = format!(r#"{{"id":{i},"result":"ok"}}"#);
                server_tx.write_all(line.as_bytes()).await.unwrap();
                server_tx.write_all(b"\n").await.unwrap();
            }
        });

        for i in 0..3 {
            let frame = transport
                .read_frame()
                .await
                .unwrap()
                .expect("frame in sequence");
            assert_eq!(frame, json!({"id": i, "result": "ok"}));
        }
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn eof_returns_none() {
        let (mut transport, mut server_tx, _server_rx) = transport_pair();

        server_tx
            .write_all(b"{\"id\":1,\"result\":1}\n")
            .await
            .unwrap();
        drop(server_tx);

        let frame = transport
            .read_frame()
            .await
            .unwrap()
            .expect("buffered frame");
        assert_eq!(frame, json!({"id": 1, "result": 1}));
        assert!(
            transport.read_frame().await.unwrap().is_none(),
            "EOF must yield Ok(None)"
        );
    }

    #[tokio::test]
    async fn malformed_blank_and_cr_lines_are_skipped() {
        let (mut transport, mut server_tx, _server_rx) = transport_pair();

        // Every line below is ≤ 64 bytes so each write_all completes without
        // needing the transport to drain concurrently.
        server_tx.write_all(b"this is not json\r\n").await.unwrap();
        server_tx.write_all(b"\xff\xfe\x00\n").await.unwrap(); // invalid UTF-8
        server_tx.write_all(b"\n").await.unwrap(); // blank line
        server_tx.write_all(b"   \t  \n").await.unwrap(); // whitespace only
        server_tx.write_all(b"{\"a\":1}\r\n").await.unwrap(); // trailing CR
        server_tx.write_all(b"{\"b\":2}\n").await.unwrap();

        let first = transport
            .read_frame()
            .await
            .unwrap()
            .expect("first valid frame");
        assert_eq!(first, json!({"a": 1}));
        let second = transport
            .read_frame()
            .await
            .unwrap()
            .expect("second valid frame");
        assert_eq!(second, json!({"b": 2}));
    }

    #[tokio::test]
    async fn oversize_line_returns_sdk_protocol_error() {
        let (client_rx, mut server_tx) = duplex(64);
        let (client_tx, _server_rx) = duplex(64);
        let mut transport = JsonRpcLineTransport::new(client_rx, client_tx);

        let writer = tokio::spawn(async move {
            let mut giant = vec![b'x'; MAX_LINE_LEN + 1];
            giant.push(b'\n');
            // The pipe only holds 64 bytes, so this blocks until the transport
            // drains ~16 MiB and trips the framing guard; dropping the
            // transport then closes the read end, surfacing as a broken pipe
            // here — ignore it.
            let _ = server_tx.write_all(&giant).await;
        });

        let err = transport.read_frame().await.unwrap_err();
        assert!(matches!(err, Error::SdkProtocol { .. }), "got {err:?}");
        assert!(
            err.is_protocol(),
            "framing guard must be an SdkProtocol error"
        );
        // FIX-10: the guard error names the observed length so callers can
        // distinguish a slightly-over line from a runaway one.
        let message = err.to_string();
        assert!(
            message.contains("observed"),
            "guard error must report the observed length, got: {message}"
        );
        let observed = message
            .split("observed ")
            .nth(1)
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<usize>().ok())
            .expect("observed length must be numeric");
        assert!(
            observed > MAX_LINE_LEN,
            "observed length {observed} must exceed the guard"
        );

        drop(transport);
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn line_exactly_at_limit_reads_successfully() {
        // FIX-10: the framing guard trips only *over* MAX_LINE_LEN; a line
        // of exactly the limit is accepted, and a valid JSON document at the
        // exact boundary still round-trips.
        let (client_rx, mut server_tx) = duplex(64);
        let (client_tx, _server_rx) = duplex(64);
        let mut transport = JsonRpcLineTransport::new(client_rx, client_tx);

        // A JSON string literal whose line totals exactly MAX_LINE_LEN bytes:
        // 2 quotes + (MAX_LINE_LEN - 2) payload bytes + trailing newline.
        let payload_len = MAX_LINE_LEN - 2;
        let writer = tokio::spawn(async move {
            server_tx.write_all(b"\"").await.unwrap();
            server_tx.write_all(&vec![b'a'; payload_len]).await.unwrap();
            server_tx.write_all(b"\"\n").await.unwrap();
        });

        let frame = transport
            .read_frame()
            .await
            .unwrap()
            .expect("a line exactly at the limit must be accepted");
        assert_eq!(
            frame.as_str().map(str::len),
            Some(payload_len),
            "the parsed value must be the full boundary-length string"
        );
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn partial_line_at_eof_is_reported() {
        // FIX-10: a valid partial line without a trailing newline at EOF is
        // still delivered (readline parity), then the stream reports EOF.
        let (client_rx, mut server_tx) = duplex(64);
        let (client_tx, _server_rx) = duplex(64);
        let mut transport = JsonRpcLineTransport::new(client_rx, client_tx);

        server_tx
            .write_all(b"{\"id\":9,\"result\":\"partial\"}")
            .await
            .unwrap();
        drop(server_tx);

        let frame = transport
            .read_frame()
            .await
            .unwrap()
            .expect("a partial valid line at EOF must be delivered");
        assert_eq!(frame, json!({"id": 9, "result": "partial"}));
        assert!(
            transport.read_frame().await.unwrap().is_none(),
            "after the partial line the stream is at EOF"
        );
    }

    #[tokio::test]
    async fn malformed_partial_line_at_eof_is_skipped() {
        // FIX-10: a malformed partial line at EOF is skipped like any other
        // malformed line, and the stream then reports EOF.
        let (client_rx, mut server_tx) = duplex(64);
        let (client_tx, _server_rx) = duplex(64);
        let mut transport = JsonRpcLineTransport::new(client_rx, client_tx);

        server_tx
            .write_all(b"not json without newline")
            .await
            .unwrap();
        drop(server_tx);

        assert!(
            transport.read_frame().await.unwrap().is_none(),
            "a malformed partial line at EOF is skipped, then EOF"
        );
    }
}
