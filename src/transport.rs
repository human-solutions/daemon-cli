use crate::terminal::TerminalInfo;
use anyhow::Result;
use futures::{SinkExt, StreamExt};
#[cfg(unix)]
use interprocess::local_socket::{GenericFilePath, ToFsName};
#[cfg(windows)]
use interprocess::local_socket::{GenericNamespaced, ToNsName};
use interprocess::local_socket::{
    ListenerOptions,
    tokio::{Listener, Stream, prelude::*},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
#[cfg(unix)]
use std::fs;
use std::{
    collections::hash_map::DefaultHasher,
    env,
    hash::{Hash, Hasher},
    path::PathBuf,
};
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

/// Cross-platform socket name for the daemon (used on Windows for named pipes).
#[cfg_attr(unix, allow(dead_code))]
pub fn socket_name(daemon_name: &str, root_path: &str) -> String {
    let short_id = hash_path_to_short_id(root_path);
    format!("{short_id}-{daemon_name}.sock")
}

/// Check if daemon socket likely exists (platform-aware).
pub fn daemon_socket_exists(daemon_name: &str, root_path: &str) -> bool {
    // On Unix, check socket file. On Windows, use PID file as proxy.
    #[cfg(unix)]
    {
        socket_path(daemon_name, root_path).exists()
    }
    #[cfg(windows)]
    {
        // Windows named pipes can't be checked via filesystem
        pid_path(daemon_name, root_path).exists()
    }
}

// Internal: Cross-platform socket server for handling RPC requests
pub struct SocketServer {
    listener: Listener,
    socket_path: PathBuf,
}

impl SocketServer {
    pub async fn new(daemon_name: &str, root_path: &str) -> Result<Self> {
        let sock_path = socket_path(daemon_name, root_path);

        // Create listener using platform-appropriate naming
        #[cfg(unix)]
        let listener = {
            // Remove existing socket file if it exists
            if sock_path.exists() {
                fs::remove_file(&sock_path)?;
            }
            ListenerOptions::new()
                .name(sock_path.clone().to_fs_name::<GenericFilePath>()?)
                .create_tokio()?
        };

        #[cfg(windows)]
        let listener = {
            let name = socket_name(daemon_name, root_path);
            ListenerOptions::new()
                .name(name.to_ns_name::<GenericNamespaced>()?)
                .create_tokio()?
        };

        // Set socket permissions to 0600 (owner read/write only) - Unix only
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&sock_path, perms)?;
        }

        Ok(Self {
            listener,
            socket_path: sock_path,
        })
    }

    pub async fn accept(&mut self) -> Result<SocketConnection> {
        let stream = self.listener.accept().await?;
        Ok(SocketConnection::new(stream))
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }
}

// Internal: Cross-platform socket client for sending RPC requests
pub struct SocketClient {
    connection: SocketConnection,
}

impl SocketClient {
    pub async fn connect(daemon_name: &str, root_path: &str) -> Result<Self> {
        // Connect using platform-appropriate naming
        #[cfg(unix)]
        let stream = {
            let sock_path = socket_path(daemon_name, root_path);
            Stream::connect(sock_path.to_fs_name::<GenericFilePath>()?).await?
        };

        #[cfg(windows)]
        let stream = {
            let name = socket_name(daemon_name, root_path);
            Stream::connect(name.to_ns_name::<GenericNamespaced>()?).await?
        };

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

// Internal: Framed connection over cross-platform socket
pub struct SocketConnection {
    framed: Framed<Stream, LengthDelimitedCodec>,
}

impl SocketConnection {
    pub fn new(stream: Stream) -> Self {
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
    VersionCheck {
        build_timestamp: u64,
    },
    Command {
        command: String,
        terminal_info: TerminalInfo,
    },
    OutputChunk(Vec<u8>),
    CommandComplete {
        exit_code: i32,
    },
    CommandError(String),
}
