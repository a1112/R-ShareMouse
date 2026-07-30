use anyhow::{Context, Result};
use bytes::Bytes;
use image::{codecs::jpeg::JpegEncoder, codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
use rshare_core::{
    DisplayCaptureBlob, DisplayCaptureDescriptor, DisplayCaptureResult, DisplayOperationStatus,
};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayCaptureEncoding {
    Png,
    Jpeg { quality: u8 },
}

#[derive(Debug)]
pub struct RawBgraFrame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn encode_bgra(frame: RawBgraFrame, encoding: DisplayCaptureEncoding) -> Result<Bytes> {
    let expected = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .context("display capture dimensions overflow")?;
    anyhow::ensure!(
        frame.pixels.len() == expected,
        "display capture BGRA length mismatch"
    );

    let mut rgba = frame.pixels;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let mut encoded = Vec::new();
    match encoding {
        DisplayCaptureEncoding::Png => PngEncoder::new(&mut encoded)
            .write_image(&rgba, frame.width, frame.height, ExtendedColorType::Rgba8)
            .context("failed to encode display capture PNG")?,
        DisplayCaptureEncoding::Jpeg { quality } => {
            let mut rgb = Vec::with_capacity((expected / 4) * 3);
            for pixel in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&pixel[..3]);
            }
            JpegEncoder::new_with_quality(&mut encoded, quality)
                .write_image(&rgb, frame.width, frame.height, ExtendedColorType::Rgb8)
                .context("failed to encode display capture JPEG")?
        }
    }
    Ok(Bytes::from(encoded))
}

pub fn success(
    display_id: impl Into<String>,
    mime_type: impl Into<String>,
    width: u32,
    height: u32,
    bytes: Bytes,
    message: impl Into<String>,
) -> DisplayCaptureResult {
    let descriptor = DisplayCaptureDescriptor {
        capture_id: Uuid::new_v4(),
        display_id: display_id.into(),
        mime_type: mime_type.into(),
        width,
        height,
        byte_length: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
    };
    DisplayCaptureResult {
        request_id: Uuid::new_v4(),
        status: DisplayOperationStatus::Success,
        message: Some(message.into()),
        payload: Some(descriptor.clone()),
        blob: Some(DisplayCaptureBlob { descriptor, bytes }),
    }
}

pub fn error(status: DisplayOperationStatus, message: impl Into<String>) -> DisplayCaptureResult {
    DisplayCaptureResult {
        request_id: Uuid::new_v4(),
        status,
        message: Some(message.into()),
        payload: None,
        blob: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_png_without_bmp_or_raw_bytes() {
        let encoded = encode_bgra(
            RawBgraFrame {
                width: 900,
                height: 506,
                pixels: vec![0; 900 * 506 * 4],
            },
            DisplayCaptureEncoding::Png,
        )
        .unwrap();
        assert_eq!(&encoded[..8], b"\x89PNG\r\n\x1a\n");
        assert!(encoded.len() < 250 * 1024);
    }

    #[test]
    fn explicitly_encodes_jpeg_at_bounded_size() {
        let encoded = encode_bgra(
            RawBgraFrame {
                width: 900,
                height: 506,
                pixels: vec![127; 900 * 506 * 4],
            },
            DisplayCaptureEncoding::Jpeg { quality: 85 },
        )
        .unwrap();
        assert_eq!(&encoded[..2], b"\xff\xd8");
        assert!(encoded.len() < 250 * 1024);
    }
}
