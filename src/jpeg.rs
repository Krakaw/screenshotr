//! BGRA -> JPEG encoding.

use jpeg_encoder::{ColorType, Encoder, EncodingError};

use crate::capture::Frame;

/// Encode a captured frame as JPEG. `quality` is 1-100.
///
/// The encoder consumes BGRA directly, so the buffer from ScreenCaptureKit
/// needs no channel-swap pass.
pub fn encode(frame: &Frame, quality: u8) -> Result<Vec<u8>, EncodingError> {
    let mut buf = Vec::new();
    let encoder = Encoder::new(&mut buf, quality);
    encoder.encode(&frame.bgra, frame.width, frame.height, ColorType::Bgra)?;
    Ok(buf)
}
