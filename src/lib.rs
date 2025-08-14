use serde::{Deserialize, Serialize};

pub trait RpcMethod: Serialize + for<'de> Deserialize<'de> + Send + Sync {
    type Response: Serialize + for<'de> Deserialize<'de> + Send + Sync;
}

#[derive(Serialize, Deserialize)]
#[serde(bound = "M: RpcMethod")]
pub struct RpcRequest<M: RpcMethod> {
    pub id: String,
    pub method: M,
    pub client_version: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(bound(deserialize = "R: serde::de::DeserializeOwned"))]
pub enum RpcResponse<R> {
    Success { output: R },
    Error { error: String },
    VersionMismatch { daemon_version: u64, message: String },
}
