//! KISS TCP server — bidirectional KISS over TCP.
//!
//! RX: decoded frames are broadcast to all connected clients.
//! TX: clients can send KISS data frames, which are forwarded to the main thread.

use tokio::net::TcpListener;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::broadcast;
use crossbeam_channel::Sender as CrossbeamSender;
use packet_radio_core::kiss;

/// Run the bidirectional KISS TCP server.
///
/// - `frame_tx`: broadcast channel for RX frames → clients
/// - `kiss_in`: crossbeam channel for client KISS bytes → main thread TX pipeline
pub async fn run_bidirectional(
    port: u16,
    frame_tx: broadcast::Sender<Vec<u8>>,
    kiss_in: CrossbeamSender<Vec<u8>>,
) {
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
                let mut rx = frame_tx.subscribe();
                let kiss_in = kiss_in.clone();
                tokio::spawn(async move {
                    let (mut reader, mut writer) = socket.into_split();
                    let mut read_buf = [0u8; 2048];

                    loop {
                        tokio::select! {
                            // RX path: broadcast frames → client
                            result = rx.recv() => {
                                match result {
                                    Ok(frame_data) => {
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
                                    Err(broadcast::error::RecvError::Closed) => break,
                                }
                            }
                            // TX path: client → main thread
                            result = reader.read(&mut read_buf) => {
                                match result {
                                    Ok(0) => break, // Client disconnected
                                    Ok(n) => {
                                        let _ = kiss_in.try_send(read_buf[..n].to_vec());
                                    }
                                    Err(e) => {
                                        tracing::debug!("KISS client {addr} read error: {e}");
                                        break;
                                    }
                                }
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

