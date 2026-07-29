//! Framed local IPC connection handling.

use std::future::Future;

use anyhow::{Context, Result};
use rshare_core::{DaemonRequest, DaemonResponse, IpcEnvelopeKind, IpcFrameCodec};
use tokio::io::{AsyncRead, AsyncWrite};

pub async fn read_json_request<S>(stream: &mut S) -> Result<Option<DaemonRequest>>
where
    S: AsyncRead + Unpin,
{
    let Some(frame) = IpcFrameCodec::default()
        .read_frame_for_kind(stream, IpcEnvelopeKind::Json)
        .await
        .context("failed to read daemon IPC frame")?
    else {
        return Ok(None);
    };
    Ok(Some(
        serde_json::from_slice(&frame.payload).context("failed to decode daemon IPC request")?,
    ))
}

pub async fn write_json_response<S>(stream: &mut S, response: &DaemonResponse) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    let payload = serde_json::to_vec(response).context("failed to encode daemon IPC response")?;
    IpcFrameCodec::default()
        .write_frame(stream, IpcEnvelopeKind::Json, &payload)
        .await
        .context("failed to write daemon IPC response")
}

pub async fn handle_persistent_json_connection<S, H, F>(mut stream: S, handler: H) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: FnMut(DaemonRequest) -> F,
    F: Future<Output = Result<DaemonResponse>>,
{
    let Some(request) = read_json_request(&mut stream).await? else {
        return Ok(());
    };
    handle_persistent_json_connection_with_first(stream, request, handler).await
}

pub async fn handle_persistent_json_connection_with_first<S, H, F>(
    mut stream: S,
    first_request: DaemonRequest,
    mut handler: H,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    H: FnMut(DaemonRequest) -> F,
    F: Future<Output = Result<DaemonResponse>>,
{
    let mut next_request = Some(first_request);
    while let Some(request) = next_request {
        if matches!(
            request,
            DaemonRequest::SubscribeLocalControls | DaemonRequest::SubscribeEndpointEvents { .. }
        ) {
            write_json_response(
                &mut stream,
                &DaemonResponse::Error(
                    "streaming subscriptions must be the first request on a dedicated connection"
                        .to_string(),
                ),
            )
            .await?;
            return Ok(());
        }
        let response = handler(request).await?;
        write_json_response(&mut stream, &response).await?;
        next_request = read_json_request(&mut stream).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use rshare_core::{read_json_frame, write_json_frame, DaemonRequest, DaemonResponse};
    use tokio::{
        io::{duplex, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };

    use super::{handle_persistent_json_connection, read_json_request};

    #[tokio::test]
    async fn ephemeral_handler_serves_back_to_back_requests_on_one_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        assert_ne!(address.port(), rshare_core::ipc::DEFAULT_IPC_PORT);
        let handled = Arc::new(AtomicUsize::new(0));
        let server_handled = Arc::clone(&handled);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_persistent_json_connection(stream, move |request| {
                let handled = Arc::clone(&server_handled);
                async move {
                    assert_eq!(request, DaemonRequest::Status);
                    handled.fetch_add(1, Ordering::Relaxed);
                    Ok(DaemonResponse::Ack)
                }
            })
            .await
            .unwrap();
        });

        let mut client = TcpStream::connect(address).await.unwrap();
        for _ in 0..2 {
            write_json_frame(&mut client, &DaemonRequest::Status)
                .await
                .unwrap();
            let response: DaemonResponse = read_json_frame(&mut client).await.unwrap();
            assert_eq!(response, DaemonResponse::Ack);
        }
        drop(client);
        server.await.unwrap();

        assert_eq!(handled.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn daemon_json_reader_rejects_binary_header_without_waiting_for_body() {
        let (mut writer, mut reader) = duplex(64);
        let mut header = (2 * 1024 * 1024_u32).to_be_bytes().to_vec();
        header.push(rshare_core::IpcEnvelopeKind::Binary as u8);
        writer.write_all(&header).await.unwrap();
        drop(writer);

        let error = read_json_request(&mut reader).await.unwrap_err();
        let io_error = error
            .downcast_ref::<std::io::Error>()
            .expect("daemon kind rejection must preserve io::Error");

        assert_eq!(io_error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn second_frame_subscription_is_rejected_without_dispatch_panic() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle_persistent_json_connection(stream, |request| async move {
                match request {
                    DaemonRequest::Status => Ok(DaemonResponse::Ack),
                    DaemonRequest::SubscribeLocalControls
                    | DaemonRequest::SubscribeEndpointEvents { .. } => {
                        panic!("streaming request reached ordinary dispatcher")
                    }
                    _ => Ok(DaemonResponse::Error("unexpected request".to_string())),
                }
            })
            .await
        });

        let mut client = TcpStream::connect(address).await.unwrap();
        write_json_frame(&mut client, &DaemonRequest::Status)
            .await
            .unwrap();
        assert_eq!(
            read_json_frame::<DaemonResponse, _>(&mut client)
                .await
                .unwrap(),
            DaemonResponse::Ack
        );

        write_json_frame(&mut client, &DaemonRequest::SubscribeLocalControls)
            .await
            .unwrap();
        let response: DaemonResponse = read_json_frame(&mut client).await.unwrap();
        let DaemonResponse::Error(message) = response else {
            panic!("expected explicit protocol error");
        };
        assert!(message.contains("first request"));
        drop(client);

        server.await.unwrap().unwrap();
    }
}
