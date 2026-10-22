use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{
    tungstenite::{
        handshake::derive_accept_key,
        protocol::{Role, WebSocketConfig},
        Message,
    },
    WebSocketStream,
};

// Authentication has already been checked by the HTTP router. Keep all input
// validation, sequence fencing, ownership and backend acknowledgement unchanged.
pub(super) async fn serve(
    mut stream: TcpStream,
    request: &MobileHttpRequest,
    sessions: &MobileClientSessions,
    network: &Arc<Mutex<NetworkManager>>,
    injection: &InputInjectionHandle,
    state: &Arc<RwLock<DaemonState>>,
    events: &broadcast::Sender<LocalInputDiagnosticEvent>,
) -> Result<()> {
    anyhow::ensure!(
        request
            .header("upgrade")
            .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
            && request.header("connection").is_some_and(|v| v
                .split(',')
                .any(|s| s.trim().eq_ignore_ascii_case("upgrade")))
            && request.header("sec-websocket-version") == Some("13"),
        "invalid input WebSocket upgrade"
    );
    let host = request.header("host").context("missing Host")?;
    anyhow::ensure!(
        request.header("origin") == Some(format!("http://{host}").as_str()),
        "input socket origin mismatch"
    );
    let key = request
        .header("sec-websocket-key")
        .context("missing WebSocket key")?;
    anyhow::ensure!(key.len() == 24, "invalid WebSocket key");
    stream.set_nodelay(true)?;
    let header = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", derive_accept_key(key.as_bytes()));
    timeout(Duration::from_secs(3), stream.write_all(header.as_bytes())).await??;
    let mut config = WebSocketConfig::default();
    config.max_message_size = Some(MAX_BODY_BYTES);
    config.max_frame_size = Some(MAX_BODY_BYTES);
    let mut socket = WebSocketStream::from_raw_socket(stream, Role::Server, Some(config)).await;
    while let Some(message) = timeout(Duration::from_secs(60), socket.next()).await? {
        let message = message?;
        let text = match message {
            Message::Text(text) => text,
            Message::Close(_) => break,
            Message::Ping(data) => {
                timeout(Duration::from_secs(2), socket.send(Message::Pong(data))).await??;
                continue;
            }
            _ => bail!("unsupported mobile input frame"),
        };
        if text == "{\"type\":\"ping\"}" {
            timeout(
                Duration::from_secs(2),
                socket.send(Message::Text("{\"type\":\"pong\"}".into())),
            )
            .await??;
            continue;
        }
        let body = mobile_inject_body_from_body(text.as_bytes())?;
        let (client_id, sequence) = match &body {
            MobileInjectBody::Single(e) => (e.client_id.clone(), e.sequence),
            MobileInjectBody::ReleaseBatch(e) => (e.client_id.clone(), e.sequence),
        };
        let result = match body {
            MobileInjectBody::Single(e) => {
                process_mobile_inject_envelope(sessions, network, injection, state, events, e)
                    .await
                    .map(|r| json!({ "EndpointInjectResult": r }))
            }
            MobileInjectBody::ReleaseBatch(e) => {
                process_mobile_release_batch(sessions, network, injection, state, events, e)
                    .await
                    .map(|r| json!({ "MobileReleaseBatchResult": { "results": r } }))
            }
        };
        let response = match result {
            Ok(result) => json!({ "client_id": client_id, "sequence": sequence, "result": result }),
            Err(error) => {
                json!({ "client_id": client_id, "sequence": sequence, "error": error.to_string() })
            }
        };
        timeout(
            Duration::from_secs(2),
            socket.send(Message::Text(response.to_string().into())),
        )
        .await??;
    }
    Ok(())
}
