//! IMAP COMPRESS=DEFLATE (RFC 4978): a byte stream that deflates what is
//! written and inflates what is read, both raw DEFLATE (RFC 1951, no zlib
//! header), layered over the TLS stream once the server accepts
//! `COMPRESS DEFLATE`.
//!
//! Every write is completed with a sync flush (zlib's Z_SYNC_FLUSH), so
//! each command reaches the server whole and decodable without waiting
//! for more data; servers flush their responses the same way (RFC 4978 §3
//! leaves the mechanics to DEFLATE, §4 discusses flush placement). The
//! layering is DEFLATE inside TLS, as §3 requires ("first COMPRESS, then
//! SASL, and finally TLS").
//! Both sides keep one compression context for the connection's life, which
//! is where the savings on mail come from (headers, HTML and quoted text
//! repeat across messages).

use std::io;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// Compressed bytes read from the peer per inner read.
const READ_CHUNK: usize = 32 * 1024;

pub struct Deflate<T> {
    inner: T,
    inflate: Decompress,
    deflate: Compress,
    /// Compressed input not yet inflated.
    input: Vec<u8>,
    input_pos: usize,
    /// Compressed output not yet written to `inner`.
    output: Vec<u8>,
    output_pos: usize,
    eof: bool,
}

impl<T> Deflate<T> {
    /// `leftover`: compressed bytes already read past the COMPRESS reply.
    pub fn new(inner: T, leftover: Vec<u8>) -> Deflate<T> {
        Deflate {
            inner,
            inflate: Decompress::new(false),
            // Level 6 (zlib's default): commands are tiny, so this only
            // matters for APPENDs, which it shrinks well.
            deflate: Compress::new(Compression::new(6), false),
            input: leftover,
            input_pos: 0,
            output: Vec::new(),
            output_pos: 0,
            eof: false,
        }
    }

    pub fn get_ref(&self) -> &T {
        &self.inner
    }
}

fn corrupt(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("compressed stream: {e}"),
    )
}

impl<T: AsyncWrite + Unpin> Deflate<T> {
    /// Write out pending compressed bytes.
    fn poll_drain(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        while self.output_pos < self.output.len() {
            let n =
                ready!(Pin::new(&mut self.inner).poll_write(cx, &self.output[self.output_pos..]))?;
            if n == 0 {
                return Poll::Ready(Err(io::ErrorKind::WriteZero.into()));
            }
            self.output_pos += n;
        }
        self.output.clear();
        self.output_pos = 0;
        Poll::Ready(Ok(()))
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Deflate<T> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        loop {
            // Even with no input left: the inflater may hold output that
            // didn't fit the caller's buffer last time (a long match).
            let (in0, out0) = (this.inflate.total_in(), this.inflate.total_out());
            let status = this
                .inflate
                .decompress(
                    &this.input[this.input_pos..],
                    buf.initialize_unfilled(),
                    FlushDecompress::None,
                )
                .map_err(corrupt)?;
            let used = (this.inflate.total_in() - in0) as usize;
            let made = (this.inflate.total_out() - out0) as usize;
            this.input_pos += used;
            buf.advance(made);
            if made > 0 {
                return Poll::Ready(Ok(()));
            }
            if status == Status::StreamEnd {
                // The peer ended the DEFLATE stream: nothing more follows.
                this.eof = true;
                return Poll::Ready(Ok(()));
            }
            if used > 0 {
                // Consumed without output (block headers, a sync flush).
                continue;
            }
            // No progress: a partial block, or nothing buffered. Read more.
            if this.eof {
                return Poll::Ready(Ok(()));
            }
            // Keep the unconsumed tail and read more after it.
            this.input.drain(..this.input_pos);
            this.input_pos = 0;
            let at = this.input.len();
            this.input.resize(at + READ_CHUNK, 0);
            let mut rb = ReadBuf::new(&mut this.input[at..]);
            let polled = Pin::new(&mut this.inner).poll_read(cx, &mut rb);
            let got = rb.filled().len();
            this.input.truncate(at + got);
            ready!(polled)?;
            if got == 0 {
                // Transport closed: inflate what is left, then EOF.
                this.eof = true;
            }
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Deflate<T> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        // Finish the previous write first, so buffered output stays bounded.
        ready!(this.poll_drain(cx))?;
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let mut consumed = 0usize;
        loop {
            if this.output.capacity() - this.output.len() < 64 {
                this.output.reserve(data.len() / 2 + 1024);
            }
            let in0 = this.deflate.total_in();
            this.deflate
                .compress_vec(&data[consumed..], &mut this.output, FlushCompress::Sync)
                .map_err(corrupt)?;
            consumed += (this.deflate.total_in() - in0) as usize;
            // A sync flush is complete once all input is in and the output
            // buffer wasn't filled to the brim.
            if consumed == data.len() && this.output.len() < this.output.capacity() {
                break;
            }
        }
        // The data is accepted; it goes out now or on the next write/flush.
        let _ = this.poll_drain(cx)?;
        Poll::Ready(Ok(data.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        ready!(this.poll_drain(cx))?;
        Pin::new(&mut this.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn round_trips_both_ways_with_small_reads() {
        let (a, b) = tokio::io::duplex(64);
        let mut a = Deflate::new(a, Vec::new());
        let mut b = Deflate::new(b, Vec::new());
        let msg: Vec<u8> = (0..200_000u32)
            .flat_map(|i| format!("line {} of the message body\r\n", i % 977).into_bytes())
            .collect();
        let expect = msg.clone();
        let writer = tokio::spawn(async move {
            for part in msg.chunks(7919) {
                a.write_all(part).await.unwrap();
                a.flush().await.unwrap();
            }
            a.write_all(b"* OK done\r\n").await.unwrap();
            a.flush().await.unwrap();
            a
        });
        let mut got = vec![0u8; expect.len() + 11];
        let mut at = 0;
        let total = got.len();
        while at < total {
            let n = b.read(&mut got[at..(at + 3).min(total)]).await.unwrap();
            assert!(n > 0, "early EOF at {at}");
            at += n;
        }
        assert_eq!(&got[..expect.len()], &expect[..]);
        assert_eq!(&got[expect.len()..], b"* OK done\r\n");
        // And back the other way on the same contexts.
        let mut a = writer.await.unwrap();
        b.write_all(b"a2 NOOP\r\n").await.unwrap();
        b.flush().await.unwrap();
        let mut line = [0u8; 9];
        a.read_exact(&mut line).await.unwrap();
        assert_eq!(&line, b"a2 NOOP\r\n");
    }

    #[tokio::test]
    async fn each_flushed_write_is_readable_without_more_data() {
        // Sync flush: the peer can decode a command before anything follows.
        let (a, b) = tokio::io::duplex(1 << 16);
        let mut a = Deflate::new(a, Vec::new());
        let mut b = Deflate::new(b, Vec::new());
        a.write_all(b"p0001 UID FETCH 1:* (FLAGS)\r\n")
            .await
            .unwrap();
        a.flush().await.unwrap();
        let mut buf = [0u8; 64];
        let n = b.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"p0001 UID FETCH 1:* (FLAGS)\r\n");
    }

    #[tokio::test]
    async fn leftover_bytes_are_inflated_first() {
        let mut c = Compress::new(Compression::new(6), false);
        let mut z = Vec::with_capacity(256);
        c.compress_vec(b"* 3 EXISTS\r\n", &mut z, FlushCompress::Sync)
            .unwrap();
        let (_a, b) = tokio::io::duplex(64);
        let mut b = Deflate::new(b, z);
        let mut buf = [0u8; 32];
        let n = b.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"* 3 EXISTS\r\n");
    }

    #[tokio::test]
    async fn garbage_is_an_error_not_a_hang() {
        let (mut a, b) = tokio::io::duplex(64);
        let mut b = Deflate::new(b, Vec::new());
        a.write_all(&[0xff; 32]).await.unwrap();
        drop(a);
        let mut buf = [0u8; 32];
        assert!(b.read(&mut buf).await.is_err());
    }
}
