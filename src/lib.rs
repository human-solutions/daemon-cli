use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub mod client;
pub mod server;

pub use client::DaemonClient;
pub use server::DaemonServer;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

pub mod prelude {
    pub use crate::*;
    pub use anyhow::Result;
    pub use async_trait::async_trait;
    pub use serde::{Deserialize, Serialize};
    pub use tokio_util::sync::CancellationToken;
}

// Core trait for RPC methods
pub trait RpcMethod: Serialize + DeserializeOwned + Send + Sync {
    type Response: Serialize + DeserializeOwned + Send + Sync + std::fmt::Debug;
}

// RPC request structure
#[derive(Serialize, Deserialize)]
#[serde(bound = "M: Serialize + DeserializeOwned")]
pub struct RpcRequest<M: RpcMethod> {
    pub method: M,
    pub client_build_timestamp: u64,
}

// RPC response structure
#[derive(Debug, Serialize, Deserialize)]
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

// Internal request envelope for server communication
pub(crate) struct RequestEnvelope<M: RpcMethod> {
    pub request: RpcRequest<M>,
    pub response_tx: oneshot::Sender<RpcResponse<M::Response>>,
    pub cancel_token: CancellationToken,
}

// Server handle for in-memory communication
pub struct ServerHandle<M: RpcMethod> {
    pub(crate) request_tx: mpsc::Sender<RequestEnvelope<M>>,
    pub(crate) status_tx: broadcast::Sender<DaemonStatus>,
}

impl<M: RpcMethod> ServerHandle<M> {
    pub fn subscribe_status(&self) -> broadcast::Receiver<DaemonStatus> {
        self.status_tx.subscribe()
    }
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
