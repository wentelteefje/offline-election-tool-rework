// src/rpc.rs
use anyhow::{Result, anyhow};
use jsonrpsee::core::client::ClientT;
use jsonrpsee::ws_client::{WsClient, WsClientBuilder};
use parity_scale_codec::Decode;

pub type Hash = [u8; 32];

/// Thin wrapper around a JSON-RPC WS client.
pub struct RpcClient {
    pub(crate) inner: WsClient,
}

impl RpcClient {
    /// Connect to a node via WebSocket.
    pub async fn connect(uri: &str) -> Result<Self> {
        let inner = WsClientBuilder::default().build(uri).await?;
        Ok(Self { inner })
    }

    /// `state_getStorage` wrapper.
    pub async fn get_storage(&self, key_hex: &str, at: Option<Hash>) -> Result<Option<Vec<u8>>> {
        let key = key_hex.to_string();

        let params = if let Some(hash) = at {
            let hash_hex = format!("0x{}", hex::encode(hash));
            jsonrpsee::rpc_params![key, hash_hex]
        } else {
            jsonrpsee::rpc_params![key]
        };

        let res: Option<String> = self.inner.request("state_getStorage", params).await?;

        let decoded = res.map(|hex_str| {
            let s = hex_str.trim_start_matches("0x");
            hex::decode(s).expect("RPC returned invalid hex")
        });

        Ok(decoded)
    }

    /// Decode storage at a key into a type `T: Decode`.
    pub async fn get_storage_decoded<T: Decode>(
        &self,
        key_hex: &str,
        at: Option<Hash>,
    ) -> Result<Option<T>> {
        if let Some(bytes) = self.get_storage(key_hex, at).await? {
            let mut slice = &bytes[..];
            let value = T::decode(&mut slice).map_err(|e| anyhow!("decode error: {:?}", e))?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// `state_getKeysPaged` wrapper that is block-aware.
    pub async fn get_keys_paged(
        &self,
        prefix_hex: &str,
        count: u32,
        start_key: Option<&str>,
        at: Option<Hash>,
    ) -> Result<Vec<String>> {
        use jsonrpsee::rpc_params;

        let keys: Vec<String> = match (start_key, at) {
            (None, None) => {
                self.inner
                    .request("state_getKeysPaged", rpc_params![prefix_hex, count])
                    .await?
            }
            (Some(start), None) => {
                self.inner
                    .request("state_getKeysPaged", rpc_params![prefix_hex, count, start])
                    .await?
            }
            (None, Some(hash)) => {
                let hash_hex = format!("0x{}", hex::encode(hash));
                let start: Option<String> = None;
                self.inner
                    .request(
                        "state_getKeysPaged",
                        rpc_params![prefix_hex, count, start, hash_hex],
                    )
                    .await?
            }
            (Some(start), Some(hash)) => {
                let hash_hex = format!("0x{}", hex::encode(hash));
                self.inner
                    .request(
                        "state_getKeysPaged",
                        rpc_params![prefix_hex, count, start, hash_hex],
                    )
                    .await?
            }
        };

        Ok(keys)
    }

    /// `chain_getBlockHash` wrapper.
    ///
    /// - `number = Some(n)` -> block hash at height `n`.
    /// - `number = None`    -> best (latest) block hash.
    pub async fn get_block_hash(&self, number: Option<u32>) -> Result<Hash> {
        let params = if let Some(n) = number {
            jsonrpsee::rpc_params![n]
        } else {
            jsonrpsee::rpc_params![]
        };

        let res: Option<String> = self.inner.request("chain_getBlockHash", params).await?;
        let hex = res.ok_or_else(|| anyhow!("chain_getBlockHash returned null"))?;

        let bytes = hex::decode(hex.trim_start_matches("0x"))?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "unexpected hash length {}, expected 32",
                bytes.len()
            ));
        }

        let mut h = [0u8; 32];
        h.copy_from_slice(&bytes);
        Ok(h)
    }
}
