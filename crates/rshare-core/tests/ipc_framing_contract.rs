use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use bytes::Bytes;
use rshare_core::{
    read_json_frame, IpcEnvelopeKind, IpcFrameCodec, DEFAULT_MAX_BINARY_FRAME_BYTES,
    DEFAULT_MAX_JSON_FRAME_BYTES, IPC_FRAME_HEADER_LEN,
};
use tokio::io::{AsyncRead, AsyncWrite};

struct CountingChunkReader {
    bytes: Vec<u8>,
    offset: usize,
    max_chunk: usize,
    read_calls: usize,
}

impl CountingChunkReader {
    fn new(bytes: Vec<u8>, max_chunk: usize) -> Self {
        Self {
            bytes,
            offset: 0,
            max_chunk,
            read_calls: 0,
        }
    }

    fn read_calls(&self) -> usize {
        self.read_calls
    }
}

impl AsyncRead for CountingChunkReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.read_calls += 1;
        if self.offset == self.bytes.len() {
            return Poll::Ready(Ok(()));
        }

        let count = self
            .max_chunk
            .min(buf.remaining())
            .min(self.bytes.len() - self.offset);
        let end = self.offset + count;
        buf.put_slice(&self.bytes[self.offset..end]);
        self.offset = end;
        Poll::Ready(Ok(()))
    }
}

#[derive(Default)]
struct FlushCountingWriter {
    bytes: Vec<u8>,
    flushes: usize,
}

impl AsyncWrite for FlushCountingWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.bytes.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.flushes += 1;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn encode_frame(kind: IpcEnvelopeKind, payload: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(IPC_FRAME_HEADER_LEN + payload.len());
    encoded.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    encoded.push(kind as u8);
    encoded.extend_from_slice(payload);
    encoded
}

#[tokio::test]
async fn fragmented_header_and_multi_megabyte_body_decode_one_frame() {
    let payload = vec![7_u8; 2 * 1024 * 1024];
    let mut source =
        CountingChunkReader::new(encode_frame(IpcEnvelopeKind::Binary, &payload), 4096);

    let frame = IpcFrameCodec::default()
        .read_frame(&mut source)
        .await
        .unwrap()
        .unwrap();

    let _: &Bytes = &frame.payload;
    assert_eq!(frame.kind, IpcEnvelopeKind::Binary);
    assert_eq!(frame.payload.as_ref(), payload);
    assert!(source.read_calls() < 600);
}

#[tokio::test]
async fn back_to_back_frames_are_decoded_without_consuming_the_next_frame() {
    let mut encoded = encode_frame(IpcEnvelopeKind::Json, br#"{"request":"one"}"#);
    encoded.extend_from_slice(&encode_frame(IpcEnvelopeKind::Heartbeat, b"two"));
    let mut source = CountingChunkReader::new(encoded, usize::MAX);
    let codec = IpcFrameCodec::default();

    let first = codec.read_frame(&mut source).await.unwrap().unwrap();
    let second = codec.read_frame(&mut source).await.unwrap().unwrap();
    let eof = codec.read_frame(&mut source).await.unwrap();

    assert_eq!(first.kind, IpcEnvelopeKind::Json);
    assert_eq!(first.payload.as_ref(), br#"{"request":"one"}"#);
    assert_eq!(second.kind, IpcEnvelopeKind::Heartbeat);
    assert_eq!(second.payload.as_ref(), b"two");
    assert!(eof.is_none());
}

#[tokio::test]
async fn oversized_frames_are_rejected_from_the_header_before_body_read() {
    for (kind, declared) in [
        (
            IpcEnvelopeKind::Json,
            DEFAULT_MAX_JSON_FRAME_BYTES as u32 + 1,
        ),
        (
            IpcEnvelopeKind::Binary,
            DEFAULT_MAX_BINARY_FRAME_BYTES as u32 + 1,
        ),
    ] {
        let mut header = declared.to_be_bytes().to_vec();
        header.push(kind as u8);
        let mut source = CountingChunkReader::new(header, IPC_FRAME_HEADER_LEN);

        let error = IpcFrameCodec::default()
            .read_frame(&mut source)
            .await
            .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(source.read_calls(), 2);
    }
}

#[tokio::test]
async fn unexpected_kind_is_rejected_before_body_read() {
    let mut source = CountingChunkReader::new(vec![0, 0, 0, 1, 0xff], IPC_FRAME_HEADER_LEN);

    let error = IpcFrameCodec::default()
        .read_frame(&mut source)
        .await
        .unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(source.read_calls(), 2);
}

#[tokio::test]
async fn json_reader_rejects_binary_kind_before_allocating_or_reading_body() {
    let mut header = (2 * 1024 * 1024_u32).to_be_bytes().to_vec();
    header.push(IpcEnvelopeKind::Binary as u8);
    let mut source = CountingChunkReader::new(header, IPC_FRAME_HEADER_LEN);

    let error = read_json_frame::<serde_json::Value, _>(&mut source)
        .await
        .unwrap_err();
    let io_error = error
        .downcast_ref::<io::Error>()
        .expect("JSON kind rejection must preserve io::Error");

    assert_eq!(io_error.kind(), io::ErrorKind::InvalidData);
    assert_eq!(source.read_calls(), 2);
}

#[tokio::test]
async fn clean_eof_and_partial_frames_have_distinct_semantics() {
    let codec = IpcFrameCodec::default();
    let mut empty = CountingChunkReader::new(Vec::new(), 1);
    assert!(codec.read_frame(&mut empty).await.unwrap().is_none());

    let mut partial_header = CountingChunkReader::new(vec![0, 0, 0], 1);
    let error = codec.read_frame(&mut partial_header).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);

    let mut partial_body =
        CountingChunkReader::new(encode_frame(IpcEnvelopeKind::Json, b"body"), usize::MAX);
    partial_body.bytes.pop();
    let error = codec.read_frame(&mut partial_body).await.unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

#[tokio::test]
async fn writer_uses_big_endian_header_and_flushes_once_per_frame() {
    let mut writer = FlushCountingWriter::default();

    IpcFrameCodec::default()
        .write_frame(&mut writer, IpcEnvelopeKind::Json, b"hello")
        .await
        .unwrap();

    assert_eq!(&writer.bytes[..4], 5_u32.to_be_bytes());
    assert_eq!(writer.bytes[4], IpcEnvelopeKind::Json as u8);
    assert_eq!(&writer.bytes[5..], b"hello");
    assert_eq!(writer.flushes, 1);
}
