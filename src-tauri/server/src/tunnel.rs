use axum::{body::Bytes, extract::ws::Message};
use kubecove_lib::{
    commands::sessions::{resolve_tunnel_target, runner::forward_pod_connection, TunnelTarget},
    models::{AppError, AppErrorKind, PortForwardRequest},
};
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

const RELAY_CHUNK_BYTES: usize = 16 * 1024;
const RELAY_QUEUE_MESSAGES: usize = 16;
const CONNECTION_ID_BYTES: usize = 36;
const BINARY_HEADER_BYTES: usize = 2 + CONNECTION_ID_BYTES;
pub(crate) const MAX_TUNNEL_PAYLOAD_BYTES: usize = RELAY_CHUNK_BYTES;
pub(crate) const MAX_TUNNEL_FRAME_BYTES: usize = BINARY_HEADER_BYTES + RELAY_CHUNK_BYTES;

#[derive(Default)]
struct TunnelState {
    forwards: HashMap<String, PortForwardRequest>,
    openings: HashMap<String, String>,
    connections: HashMap<String, TunnelConnection>,
}

struct TunnelConnection {
    forward_id: String,
    incoming: Option<mpsc::Sender<Vec<u8>>>,
    task: JoinHandle<()>,
}

/// An owner-scoped registry of laptop port-forward requests and active byte streams.
#[derive(Clone, Default)]
pub struct TunnelRegistry {
    state: Arc<Mutex<TunnelState>>,
}

impl TunnelRegistry {
    pub(crate) fn close_connections(&self) {
        let mut state = self.state.lock().expect("tunnel registry lock");
        state.openings.clear();
        for (_, connection) in state.connections.drain() {
            connection.task.abort();
        }
    }
    pub fn close_all(&self) {
        let mut state = self.state.lock().expect("tunnel registry lock");
        state.forwards.clear();
        state.openings.clear();
        for (_, connection) in state.connections.drain() {
            connection.task.abort();
        }
    }

    pub(crate) async fn register(
        &self,
        forward_id: String,
        request: PortForwardRequest,
    ) -> Result<TunnelTarget, AppError> {
        if !valid_id(&forward_id) {
            return Err(tunnel_validation("forward id must be a canonical UUID"));
        }
        let target = resolve_tunnel_target(request.clone()).await?;
        let mut state = self.state.lock().expect("tunnel registry lock");
        if state.forwards.contains_key(&forward_id) {
            return Err(tunnel_session("port-forward id is already registered"));
        }
        state.forwards.insert(forward_id, request);
        Ok(target)
    }

    pub(crate) async fn open(
        &self,
        forward_id: &str,
        connection_id: String,
        outbound: mpsc::Sender<Message>,
    ) -> Result<(), AppError> {
        if !valid_id(&connection_id) {
            return Err(tunnel_validation("connection id must be a canonical UUID"));
        }
        let request = {
            let mut state = self.state.lock().expect("tunnel registry lock");
            let request = state
                .forwards
                .get(forward_id)
                .cloned()
                .ok_or_else(|| tunnel_session("port-forward id is stale or unknown"))?;
            if state.connections.contains_key(&connection_id)
                || state.openings.contains_key(&connection_id)
            {
                return Err(tunnel_session("connection id is already open"));
            }
            state
                .openings
                .insert(connection_id.clone(), forward_id.to_string());
            request
        };

        let target = match resolve_tunnel_target(request).await {
            Ok(target) => target,
            Err(error) => {
                self.remove_opening(&connection_id);
                return Err(error);
            }
        };

        let (incoming, incoming_rx) = mpsc::channel(RELAY_QUEUE_MESSAGES);
        let (start, started) = oneshot::channel();
        let registry = self.clone();
        let connection_id_for_task = connection_id.clone();
        let forward_id_for_task = forward_id.to_string();
        let mut task = Some(tokio::spawn(async move {
            if started.await.is_err() {
                return;
            }
            let result = Box::pin(relay_connection(
                target,
                incoming_rx,
                outbound.clone(),
                connection_id_for_task.clone(),
            ))
            .await;
            let message = match result {
                Ok(()) => {
                    json!({ "type": "tunnel_closed", "connectionId": connection_id_for_task })
                }
                Err(error) => json!({
                    "type": "tunnel_error",
                    "forwardId": forward_id_for_task,
                    "connectionId": connection_id_for_task,
                    "error": error,
                }),
            };
            let _ = outbound
                .send(Message::Text(message.to_string().into()))
                .await;
            registry.remove_connection(&connection_id_for_task, false);
        }));

        let inserted = {
            let mut state = self.state.lock().expect("tunnel registry lock");
            let opening_matches = state
                .openings
                .get(&connection_id)
                .is_some_and(|id| id == forward_id);
            let forward_is_current = state.forwards.contains_key(forward_id);
            if opening_matches && forward_is_current {
                state.openings.remove(&connection_id);
                let Some(task) = task.take() else {
                    return Err(tunnel_session("connection task could not be started"));
                };
                state.connections.insert(
                    connection_id.clone(),
                    TunnelConnection {
                        forward_id: forward_id.to_string(),
                        incoming: Some(incoming),
                        task,
                    },
                );
                true
            } else {
                false
            }
        };
        if !inserted {
            if let Some(task) = task {
                task.abort();
            }
            return Err(tunnel_session(
                "port-forward was closed before the connection opened",
            ));
        }
        let _ = start.send(());
        Ok(())
    }

    pub(crate) async fn send_binary(&self, frame: &[u8]) -> Result<Option<String>, AppError> {
        let (connection_id, payload) = parse_binary_frame(frame)
            .ok_or_else(|| tunnel_validation("invalid tunnel data frame"))?;
        let sender = self
            .state
            .lock()
            .expect("tunnel registry lock")
            .connections
            .get(connection_id)
            .and_then(|connection| connection.incoming.clone());
        let Some(sender) = sender else {
            self.close_connection(connection_id);
            return Ok(Some(connection_id.to_string()));
        };
        if sender.send(payload.to_vec()).await.is_err() {
            self.close_connection(connection_id);
            return Ok(Some(connection_id.to_string()));
        }
        Ok(None)
    }

    pub(crate) fn unregister(&self, forward_id: &str) -> bool {
        let (removed, connections) = {
            let mut state = self.state.lock().expect("tunnel registry lock");
            let removed = state.forwards.remove(forward_id).is_some();
            state.openings.retain(|_, id| id != forward_id);
            let ids = state
                .connections
                .iter()
                .filter(|(_, connection)| connection.forward_id == forward_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let connections = ids
                .into_iter()
                .filter_map(|id| state.connections.remove(&id))
                .collect::<Vec<_>>();
            (removed, connections)
        };
        for connection in connections {
            connection.task.abort();
        }
        removed
    }

    pub(crate) fn close_connection(&self, connection_id: &str) -> bool {
        let connection = self.remove_connection(connection_id, true);
        connection.is_some()
    }

    pub(crate) fn finish_upload(&self, connection_id: &str) -> bool {
        let mut state = self.state.lock().expect("tunnel registry lock");
        let Some(connection) = state.connections.get_mut(connection_id) else {
            return false;
        };
        connection.incoming.take();
        true
    }

    fn remove_opening(&self, connection_id: &str) {
        self.state
            .lock()
            .expect("tunnel registry lock")
            .openings
            .remove(connection_id);
    }

    fn remove_connection(&self, connection_id: &str, abort: bool) -> Option<TunnelConnection> {
        let connection = self
            .state
            .lock()
            .expect("tunnel registry lock")
            .connections
            .remove(connection_id);
        if abort {
            if let Some(connection) = &connection {
                connection.task.abort();
            }
        }
        connection
    }
}

async fn relay_connection(
    target: kubecove_lib::commands::sessions::TunnelTarget,
    mut incoming: mpsc::Receiver<Vec<u8>>,
    outbound: mpsc::Sender<Message>,
    connection_id: String,
) -> Result<(), AppError> {
    let (desktop_side, kube_side) = tokio::io::duplex(RELAY_CHUNK_BYTES);
    let (mut desktop_read, mut desktop_write) = tokio::io::split(desktop_side);
    let forward = forward_pod_connection(
        target.client,
        target.namespace,
        target.pod_name,
        target.pod_port,
        kube_side,
    );
    tokio::pin!(forward);

    let upload = async {
        while let Some(payload) = incoming.recv().await {
            desktop_write.write_all(&payload).await.map_err(io_error)?;
        }
        desktop_write.shutdown().await.map_err(io_error)
    };
    let download = async {
        let mut payload = [0_u8; RELAY_CHUNK_BYTES];
        loop {
            let size = desktop_read.read(&mut payload).await.map_err(io_error)?;
            if size == 0 {
                return Ok::<(), AppError>(());
            }
            let mut frame = Vec::with_capacity(BINARY_HEADER_BYTES + size);
            frame.extend_from_slice(
                &u16::try_from(CONNECTION_ID_BYTES)
                    .expect("UUID length fits u16")
                    .to_be_bytes(),
            );
            frame.extend_from_slice(connection_id.as_bytes());
            frame.extend_from_slice(&payload[..size]);
            outbound
                .send(Message::Binary(Bytes::from(frame)))
                .await
                .map_err(|_| AppError::new("remote tunnel socket closed", AppErrorKind::Session))?;
        }
    };

    tokio::pin!(upload);
    tokio::pin!(download);
    let transfer = async {
        tokio::select! {
            result = &mut upload => match result {
                Ok(()) => download.await,
                Err(error) => Err(error),
            },
            result = &mut download => result,
        }
    };
    tokio::pin!(transfer);
    tokio::select! {
        result = &mut forward => match result {
            // The duplex read half can still contain response bytes after forwarding ends.
            Ok(()) => transfer.await,
            Err(error) => Err(error),
        },
        result = &mut transfer => result,
    }
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::new("tunnel relay I/O failed", AppErrorKind::Io).with_source(error)
}

pub(crate) fn valid_id(value: &str) -> bool {
    value.len() == CONNECTION_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            if [8, 13, 18, 23].contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}

pub(crate) fn parse_binary_frame(frame: &[u8]) -> Option<(&str, &[u8])> {
    if frame.len() < 2 {
        return None;
    }
    let id_length = usize::from(u16::from_be_bytes([frame[0], frame[1]]));
    if id_length != CONNECTION_ID_BYTES || frame.len() < 2 + id_length {
        return None;
    }
    let payload = &frame[2 + id_length..];
    if payload.len() > MAX_TUNNEL_PAYLOAD_BYTES {
        return None;
    }
    let connection_id = std::str::from_utf8(&frame[2..2 + id_length]).ok()?;
    valid_id(connection_id).then_some((connection_id, payload))
}

fn tunnel_validation(message: &str) -> AppError {
    AppError::new(message, AppErrorKind::Validation)
}

fn tunnel_session(message: &str) -> AppError {
    AppError::new(message, AppErrorKind::Session)
}

#[cfg(test)]
mod tests;
