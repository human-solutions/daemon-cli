use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub mod prelude {
    pub use crate::*;
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use serde::{Deserialize, Serialize};
    pub use tokio_util::sync::CancellationToken;
}

// Core trait for RPC methods
pub trait RpcMethod: Serialize + DeserializeOwned + Send + Sync {
    type Response: Serialize + DeserializeOwned + Send + Sync;
}

// RPC request structure
#[derive(Serialize, Deserialize)]
#[serde(bound = "M: Serialize + DeserializeOwned")]
pub struct RpcRequest<M: RpcMethod> {
    pub method: M,
    pub client_build_timestamp: u64,
}

// RPC response structure
#[derive(Serialize, Deserialize)]
pub enum RpcResponse<R> {
    Success { output: R },
    Error { error: String },
    VersionMismatch { daemon_build_timestamp: u64 },
}

// Daemon status enumeration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DaemonStatus {
    Ready,
    Busy(String),  // User message describing what the daemon is doing
    Error(String), // Error message describing why the daemon cannot run
}

// Handler trait for daemon implementations
#[async_trait]
pub trait RpcHandler<M: RpcMethod>: Send + Sync {
    async fn handle(
        &mut self,
        method: M,
        cancel_token: CancellationToken,
        status_tx: mpsc::Sender<DaemonStatus>,
    ) -> Result<M::Response>;
}

// Daemon client for communicating with daemon
pub struct DaemonClient<M: RpcMethod> {
    pub status_receiver: mpsc::Receiver<DaemonStatus>,
    pub daemon_id: u64,
    pub daemon_executable: PathBuf,
    pub build_timestamp: u64,
    _phantom: std::marker::PhantomData<M>,
}

impl<M: RpcMethod> DaemonClient<M> {
    pub async fn connect(
        daemon_id: u64,
        daemon_executable: PathBuf,
        build_timestamp: u64,
    ) -> Result<Self> {
        let (_status_tx, status_receiver) = mpsc::channel(32);

        Ok(Self {
            status_receiver,
            daemon_id,
            daemon_executable,
            build_timestamp,
            _phantom: std::marker::PhantomData,
        })
    }

    pub async fn request(&mut self, _method: M) -> Result<RpcResponse<M::Response>> {
        todo!("Implement request sending logic")
    }

    pub async fn cancel(&mut self) -> Result<()> {
        todo!("Implement cancel logic")
    }
}

// Daemon server implementation
pub struct DaemonServer<H, M: RpcMethod> {
    pub daemon_id: u64,
    handler: H,
    _phantom: std::marker::PhantomData<M>,
}

impl<H, M> DaemonServer<H, M>
where
    H: RpcHandler<M>,
    M: RpcMethod,
{
    pub fn new(daemon_id: u64, handler: H) -> Self {
        Self {
            daemon_id,
            handler,
            _phantom: std::marker::PhantomData,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        todo!("Implement daemon server main loop")
    }
}
