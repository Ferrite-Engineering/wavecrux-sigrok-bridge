//! Length-prefixed framing for the IPC protocol.
//!
//! See the crate-level doc comment for the wire format. This module
//! implements the synchronous read/write codec on top of any
//! [`std::io::Read`] / [`std::io::Write`].

use std::io::{Read, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use crate::MAX_MESSAGE_BYTES;

/// Codec-level errors. Higher layers convert these into
/// protocol-level errors that the peer can interpret.
#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("peer closed the pipe")]
    PeerClosed,

    #[error("frame too large: {actual} bytes (limit {limit})")]
    FrameTooLarge { actual: u32, limit: u32 },
}

/// Reads length-prefixed frames from any [`Read`].
pub struct FrameReader<R: Read> {
    inner: R,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Read one frame from the stream and return its body bytes.
    ///
    /// Returns [`CodecError::PeerClosed`] when the underlying reader
    /// reports EOF before a frame begins. Mid-frame EOF is reported as
    /// a generic [`CodecError::Io`].
    pub fn read_frame(&mut self) -> Result<Vec<u8>, CodecError> {
        let len = match self.inner.read_u32::<LittleEndian>() {
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(CodecError::PeerClosed);
            }
            Err(e) => return Err(CodecError::Io(e)),
        };
        if len > MAX_MESSAGE_BYTES {
            return Err(CodecError::FrameTooLarge {
                actual: len,
                limit: MAX_MESSAGE_BYTES,
            });
        }
        let mut buf = vec![0u8; len as usize];
        self.inner.read_exact(&mut buf)?;
        Ok(buf)
    }
}

/// Writes length-prefixed frames to any [`Write`].
pub struct FrameWriter<W: Write> {
    inner: W,
}

impl<W: Write> FrameWriter<W> {
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Write a single frame whose body is `body`.
    pub fn write_frame(&mut self, body: &[u8]) -> Result<(), CodecError> {
        let len = u32::try_from(body.len()).map_err(|_| CodecError::FrameTooLarge {
            actual: u32::MAX,
            limit: MAX_MESSAGE_BYTES,
        })?;
        if len > MAX_MESSAGE_BYTES {
            return Err(CodecError::FrameTooLarge {
                actual: len,
                limit: MAX_MESSAGE_BYTES,
            });
        }
        self.inner.write_u32::<LittleEndian>(len)?;
        self.inner.write_all(body)?;
        self.inner.flush()?;
        Ok(())
    }

    /// Borrow the underlying writer for advanced use.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_a_single_frame() {
        let body = b"hello world";
        let mut buf: Vec<u8> = Vec::new();
        let mut w = FrameWriter::new(&mut buf);
        w.write_frame(body).unwrap();

        let mut r = FrameReader::new(Cursor::new(buf));
        let got = r.read_frame().unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn round_trips_three_back_to_back_frames() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = FrameWriter::new(&mut buf);
            w.write_frame(b"first").unwrap();
            w.write_frame(b"").unwrap();
            w.write_frame(b"third frame, larger payload").unwrap();
        }
        let mut r = FrameReader::new(Cursor::new(buf));
        assert_eq!(r.read_frame().unwrap(), b"first");
        assert_eq!(r.read_frame().unwrap(), b"");
        assert_eq!(r.read_frame().unwrap(), b"third frame, larger payload");
        // Subsequent read returns peer-closed.
        assert!(matches!(
            r.read_frame().unwrap_err(),
            CodecError::PeerClosed
        ));
    }

    #[test]
    fn rejects_oversize_frame_at_read() {
        // Manually construct a length prefix that exceeds the cap.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&(MAX_MESSAGE_BYTES + 1).to_le_bytes());
        // No body needs to follow; the reader fails on the length.
        let mut r = FrameReader::new(Cursor::new(buf));
        let err = r.read_frame().unwrap_err();
        assert!(matches!(err, CodecError::FrameTooLarge { .. }));
    }

    #[test]
    fn write_then_read_preserves_unicode() {
        let body = "annotation: 日本語 + ñ + 🚀".as_bytes();
        let mut buf: Vec<u8> = Vec::new();
        FrameWriter::new(&mut buf).write_frame(body).unwrap();
        let got = FrameReader::new(Cursor::new(buf)).read_frame().unwrap();
        assert_eq!(got, body);
    }
}
