//! Frames on the wire: a 4-byte big-endian length, then that many bytes.

use crate::{PairError, Result};

/// Ten accounts' tokens and names are a few KB; this is generous.
pub const MAX_FRAME: usize = 256 * 1024;
pub const HEADER_LEN: usize = 4;

/// Length prefix + body.
pub fn encode(body: &[u8]) -> Result<Vec<u8>> {
    if body.len() > MAX_FRAME {
        return Err(PairError::TooLarge);
    }
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

/// The body length a header announces (refusing oversized frames before
/// reading them).
pub fn body_len(header: &[u8]) -> Result<usize> {
    let header: [u8; HEADER_LEN] = header
        .try_into()
        .map_err(|_| PairError::Protocol("bad frame header".into()))?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME {
        return Err(PairError::TooLarge);
    }
    Ok(len)
}

#[cfg(feature = "tokio")]
pub async fn read<R: tokio::io::AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; HEADER_LEN];
    reader.read_exact(&mut header).await.map_err(io_error)?;
    let mut body = vec![0u8; body_len(&header)?];
    reader.read_exact(&mut body).await.map_err(io_error)?;
    Ok(body)
}

#[cfg(feature = "tokio")]
pub async fn write<W: tokio::io::AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    writer.write_all(&encode(body)?).await.map_err(io_error)?;
    writer.flush().await.map_err(io_error)
}

#[cfg(feature = "tokio")]
fn io_error(e: std::io::Error) -> PairError {
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof
        | std::io::ErrorKind::ConnectionReset
        | std::io::ErrorKind::BrokenPipe
        | std::io::ErrorKind::ConnectionAborted => PairError::Disconnected,
        _ => PairError::Io(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_and_bounds() {
        let framed = encode(b"hi").unwrap();
        assert_eq!(framed, [0, 0, 0, 2, b'h', b'i']);
        assert_eq!(body_len(&framed[..4]).unwrap(), 2);
        assert_eq!(
            body_len(&((MAX_FRAME as u32 + 1).to_be_bytes())),
            Err(PairError::TooLarge)
        );
        assert!(body_len(&[0, 1]).is_err());
        assert_eq!(encode(&vec![0; MAX_FRAME + 1]), Err(PairError::TooLarge));
    }
}
