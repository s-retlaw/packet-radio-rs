use axum::{
    extract::{ws::WebSocket, State, WebSocketUpgrade},
    response::IntoResponse,
};
use futures::{SinkExt, StreamExt};
use std::sync::atomic::{AtomicU32, Ordering};

use super::state::AppState;
use crate::models::WsEvent;

static WS_CONNECTIONS: AtomicU32 = AtomicU32::new(0);
const MAX_WS_CONNECTIONS: u32 = 50;

/// WebSocket upgrade handler for `/ws/packets`.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> axum::response::Response {
    if WS_CONNECTIONS.load(Ordering::Relaxed) >= MAX_WS_CONNECTIONS {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    ws.max_message_size(65536)
        .on_upgrade(|socket| handle_socket(socket, state))
        .into_response()
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    WS_CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    let (mut sender, mut receiver) = socket.split();

    // Send init message with recent packets
    let recent = match super::db::get_recent_packets(&state.db, 50).await {
        Ok(packets) => packets,
        Err(e) => {
            tracing::error!("Failed to get recent packets for WS init: {}", e);
            vec![]
        }
    };

    let init_event = WsEvent::Init { packets: recent };
    if let Ok(json) = serde_json::to_string(&init_event) {
        if sender
            .send(axum::extract::ws::Message::Text(json.into()))
            .await
            .is_err()
        {
            return;
        }
    }

    // Subscribe to broadcast channel
    let mut rx = state.packet_tx.subscribe();

    // Forward broadcast events to WebSocket client
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if sender
                .send(axum::extract::ws::Message::Text(msg.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    // Read from client (just consume to detect disconnect)
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(_msg)) = receiver.next().await {
            // Ignore client messages for now
        }
    });

    // Wait for either task to finish (client disconnect or broadcast end)
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
    WS_CONNECTIONS.fetch_sub(1, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use crate::models::{PacketRow, WsEvent};

    #[test]
    fn test_ws_event_packet_serialization() {
        let event = WsEvent::Packet(PacketRow {
            id: 42,
            source: "N0CALL".into(),
            source_ssid: 0,
            dest: "APRS".into(),
            path: Some("WIDE1-1".into()),
            packet_type: Some("Position".into()),
            raw_info: "!4903.50N/07201.75W-".into(),
            summary: Some("49.058N, 72.030W".into()),
            received_at: "2026-03-01T12:00:00Z".into(),
            source_type: "tnc".into(),
        });

        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"Packet\""));
        assert!(json.contains("\"source\":\"N0CALL\""));

        let back: WsEvent = serde_json::from_str(&json).unwrap();
        match back {
            WsEvent::Packet(p) => {
                assert_eq!(p.source, "N0CALL");
                assert_eq!(p.id, 42);
            }
            _ => panic!("Expected Packet"),
        }
    }

    #[test]
    fn test_ws_event_init_serialization() {
        let event = WsEvent::Init { packets: vec![] };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"Init\""));
        assert!(json.contains("\"packets\":[]"));
    }
}
