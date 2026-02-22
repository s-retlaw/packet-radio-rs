//! KISS TCP server — serves decoded frames to connected KISS clients.

use tokio::net::TcpListener;
use tokio::io::AsyncWriteExt;
use tokio::sync::broadcast;
use packet_radio_core::kiss;

/// Run the KISS TCP server.
///
/// Each connected client gets its own subscription to the frame broadcast.
pub async fn run_with_sender(port: u16, tx: broadcast::Sender<Vec<u8>>) {
    let listener = match TcpListener::bind(format!("0.0.0.0:{port}")).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("KISS TCP bind failed on port {port}: {e}");
            return;
        }
    };

    tracing::info!("KISS TCP server listening on port {port}");

    loop {
        match listener.accept().await {
            Ok((socket, addr)) => {
                tracing::info!("KISS client connected from {addr}");
                let mut rx = tx.subscribe();
                tokio::spawn(async move {
                    let (_, mut writer) = socket.into_split();
                    loop {
                        match rx.recv().await {
                            Ok(frame_data) => {
                                // KISS-encode the frame
                                let mut kiss_buf = [0u8; 1024];
                                if let Some(len) = kiss::encode_frame(0, &frame_data, &mut kiss_buf) {
                                    if let Err(e) = writer.write_all(&kiss_buf[..len]).await {
                                        tracing::debug!("KISS client {addr} write error: {e}");
                                        break;
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("KISS client {addr} lagged, dropped {n} frames");
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                    tracing::info!("KISS client {addr} disconnected");
                });
            }
            Err(e) => {
                tracing::error!("KISS accept error: {e}");
            }
        }
    }
}
