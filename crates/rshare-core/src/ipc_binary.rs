//! Correlated binary payloads carried after bounded JSON IPC metadata.

use std::io;

use bytes::{Bytes, BytesMut};

use crate::{
    DisplayCaptureDescriptor, DisplayCaptureResult, IpcEnvelopeKind,
    DEFAULT_MAX_BINARY_FRAME_BYTES, DEFAULT_MAX_JSON_FRAME_BYTES,
};

pub const DISPLAY_CAPTURE_ID_BYTES: usize = 16;

pub fn encode_display_capture_binary(
    descriptor: &DisplayCaptureDescriptor,
    image: Bytes,
) -> io::Result<Bytes> {
    let image_len = u32::try_from(image.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "display capture is too large"))?;
    if image_len != descriptor.byte_length {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "display capture byte length mismatch",
        ));
    }
    let total_len = DISPLAY_CAPTURE_ID_BYTES
        .checked_add(image.len())
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "display capture is too large")
        })?;
    if total_len > DEFAULT_MAX_BINARY_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "display capture exceeds binary frame limit",
        ));
    }
    let mut body = BytesMut::with_capacity(total_len);
    body.extend_from_slice(descriptor.capture_id.as_bytes());
    body.extend_from_slice(&image);
    Ok(body.freeze())
}

pub fn decode_display_capture_binary(
    descriptor: &DisplayCaptureDescriptor,
    body: Bytes,
) -> io::Result<Bytes> {
    let expected_len = DISPLAY_CAPTURE_ID_BYTES
        .checked_add(descriptor.byte_length as usize)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "display capture is too large")
        })?;
    if body.len() != expected_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "display capture byte length mismatch",
        ));
    }
    if body[..DISPLAY_CAPTURE_ID_BYTES] != descriptor.capture_id.as_bytes()[..] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "display capture id mismatch",
        ));
    }
    Ok(body.slice(DISPLAY_CAPTURE_ID_BYTES..))
}

pub fn encode_display_capture_response(result: &DisplayCaptureResult) -> io::Result<Bytes> {
    let json = serde_json::to_vec(result)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if json.len() > DEFAULT_MAX_JSON_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "display capture metadata exceeds JSON frame limit",
        ));
    }

    let binary = match (&result.status, &result.payload, &result.blob) {
        (&crate::DisplayOperationStatus::Success, Some(descriptor), Some(blob))
            if descriptor == &blob.descriptor =>
        {
            Some(encode_display_capture_binary(
                descriptor,
                blob.bytes.clone(),
            )?)
        }
        (&crate::DisplayOperationStatus::Success, _, _) => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "successful display capture requires matching metadata and binary",
            ))
        }
        (_, None, None) => None,
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "failed display capture must not contain binary data",
            ))
        }
    };
    let capacity = 5 + json.len() + binary.as_ref().map_or(0, |body| 5 + body.len());
    let mut response = BytesMut::with_capacity(capacity);
    append_frame(&mut response, IpcEnvelopeKind::Json, &json)?;
    if let Some(body) = binary {
        append_frame(&mut response, IpcEnvelopeKind::Binary, &body)?;
    }
    Ok(response.freeze())
}

fn append_frame(output: &mut BytesMut, kind: IpcEnvelopeKind, payload: &[u8]) -> io::Result<()> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "IPC frame is too large"))?;
    output.extend_from_slice(&len.to_be_bytes());
    output.extend_from_slice(&[kind as u8]);
    output.extend_from_slice(payload);
    Ok(())
}
