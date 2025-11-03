use anyhow::Result;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{collections::hash_map::DefaultHasher, env, fs, hash::{Hash, Hasher}, path::PathBuf};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

// Base62 character set for encoding
const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

// Internal: Hash a path and convert to 4-character base62 identifier
fn hash_path_to_short_id(path: &str) -> String {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    let hash = hasher.finish();

    // Convert to base62 and take first 4 characters
    let mut num = hash;
    let mut result = Vec::new();

    for _ in 0..4 {
        result.push(BASE62[(num % 62) as usize]);
        num /= 62;
    }

    String::from_utf8(result).unwrap()
}

// Internal: Generate socket path for a daemon
pub fn socket_path(daemon_name: &str, root_path: &str) -> PathBuf {
    let short_id = hash_path_to_short_id(root_path);
    let temp_dir = env::temp_dir();
    temp_dir.join(format!("{short_id}-{daemon_name}.sock"))
}

// Internal: Generate PID file path for a daemon
pub fn pid_path(daemon_name: &str, root_path: &str) -> PathBuf {
    let short_id = hash_path_to_short_id(root_path);
    let temp_dir = env::temp_dir();
    temp_dir.join(format!("{short_id}-{daemon_name}.pid"))
}

// Internal: Serialize a message to bytes
pub fn serialize_message<T: Serialize>(message: &T) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(message)?;
    Ok(json)
}

// Internal: Deserialize bytes to a message
pub fn deserialize_message<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let message = serde_json::from_slice(bytes)?;
    Ok(message)
}

// Internal: Unix socket server for handling RPC requests
pub struct SocketServer {
    listener: UnixListener,
    socket_path: PathBuf,
}

impl SocketServer {
    pub async fn new(daemon_name: &str, root_path: &str) -> Result<Self> {
        let socket_path = socket_path(daemon_name, root_path);

        // Remove existing socket file if it exists
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;

        // Set socket permissions to 0600 (owner read/write only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&socket_path, perms)?;
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
        let _ = fs::remove_file(&self.socket_path);
    }
}

// Internal: Unix socket client for sending RPC requests
pub struct SocketClient {
    connection: SocketConnection,
}

impl SocketClient {
    pub async fn connect(daemon_name: &str, root_path: &str) -> Result<Self> {
        let socket_path = socket_path(daemon_name, root_path);
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self {
            connection: SocketConnection::new(stream),
        })
    }

    pub async fn send_message<T: Serialize>(&mut self, message: &T) -> Result<()> {
        self.connection.send_message(message).await
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.connection.flush().await
    }

    pub async fn receive_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        self.connection.receive_message().await
    }
}

// Internal: Framed connection over Unix socket
pub struct SocketConnection {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
}

impl SocketConnection {
    pub fn new(stream: UnixStream) -> Self {
        let codec = LengthDelimitedCodec::new();
        let framed = Framed::new(stream, codec);
        Self { framed }
    }

    pub async fn send_message<T: Serialize>(&mut self, message: &T) -> Result<()> {
        let bytes = serialize_message(message)?;
        self.framed.send(bytes.into()).await?;
        Ok(())
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.framed.flush().await?;
        Ok(())
    }

    pub async fn receive_message<T: DeserializeOwned>(&mut self) -> Result<Option<T>> {
        if let Some(bytes) = self.framed.next().await {
            let bytes = bytes?;
            let message = deserialize_message(&bytes[..])?;
            Ok(Some(message))
        } else {
            Ok(None)
        }
    }
}

// Internal: Message types for socket communication
#[derive(Serialize, Deserialize, Debug)]
pub enum SocketMessage {
    VersionCheck { build_timestamp: u64 },
    Command(String),
    OutputChunk(Vec<u8>),
    CommandComplete { exit_code: i32 },
    CommandError(String),
}
