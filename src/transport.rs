use crate::*;
use anyhow::Result;
use bytes::Bytes;
use futures::{SinkExt, StreamExt};
use std::path::PathBuf;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Generate socket path for a daemon ID
pub fn socket_path(daemon_id: u64) -> PathBuf {
    let temp_dir = std::env::temp_dir();
    temp_dir.join(format!("daemon-rpc-{daemon_id}.sock"))
}

/// Serialize a message to bytes
pub fn serialize_message<T: serde::Serialize>(message: &T) -> Result<Bytes> {
    let json = serde_json::to_vec(message)?;
    Ok(Bytes::from(json))
}

/// Deserialize bytes to a message
pub fn deserialize_message<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let message = serde_json::from_slice(bytes)?;
    Ok(message)
}

/// Unix socket server for handling RPC requests
pub struct SocketServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl SocketServer {
    pub async fn new(daemon_id: u64) -> Result<Self> {
        let socket_path = socket_path(daemon_id);

        // Remove existing socket file if it exists
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;

        // Set socket permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&socket_path, perms)?;
        }

        Ok(Self {
            listener,
            socket_path,
        })
    }

    pub async fn accept(&mut self) -> Result<SocketConnection> {
        let (stream, _addr) = self.listener.accept().await?;
        Ok(SocketConnection::new(stream))
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }
}

impl Drop for SocketServer {
    fn drop(&mut self) {
        // Clean up socket file
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Unix socket client for sending RPC requests
pub struct SocketClient {
    connection: SocketConnection,
}

impl SocketClient {
    pub async fn connect(daemon_id: u64) -> Result<Self> {
        let socket_path = socket_path(daemon_id);
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self {
            connection: SocketConnection::new(stream),
        })
    }

    pub async fn send_message<T: serde::Serialize>(&mut self, message: &T) -> Result<()> {
        self.connection.send_message(message).await
    }

    pub async fn receive_message<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>> {
        self.connection.receive_message().await
    }
}

/// Framed connection over Unix socket
pub struct SocketConnection {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
}

impl SocketConnection {
    pub fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::new();
        let framed = Framed::new(stream, codec);
        Self { framed }
    }

    pub async fn send_message<T: serde::Serialize>(&mut self, message: &T) -> Result<()> {
        let bytes = serialize_message(message)?;
        self.framed.send(bytes).await?;
        Ok(())
    }

    pub async fn receive_message<T: serde::de::DeserializeOwned>(&mut self) -> Result<Option<T>> {
        if let Some(bytes) = self.framed.next().await {
            let bytes = bytes?;
            let message = deserialize_message(&bytes[..])?;
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}

/// Message types for socket communication
#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(bound = "M: RpcMethod + Serialize + DeserializeOwned")]
pub enum SocketMessage<M: RpcMethod> {
    Request(RpcRequest<M>),
    Response(RpcResponse<M::Response>),
    Cancel,
    CancelAck,
}

/// Non-generic status message for broadcasting
#[derive(serde::Serialize, serde::Deserialize, Debug)]
pub enum StatusMessage {
    Status(DaemonStatus),
}
