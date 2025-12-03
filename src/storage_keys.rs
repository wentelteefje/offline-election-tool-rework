// src/storage_keys.rs
use crate::rpc::RpcClient;
use crate::types::Hash;
use anyhow::{Result, anyhow};
use sp_core::hashing::twox_128;

/// 32-byte prefix = `Twox128("Module") ++ Twox128("StorageItem")`.
pub fn plain_prefix(module: &str, storage: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&twox_128(module.as_bytes()));
    out[16..].copy_from_slice(&twox_128(storage.as_bytes()));
    out
}

pub fn plain_key_hex(module: &str, storage: &str) -> String {
    let prefix = plain_prefix(module, storage);
    format!("0x{}", hex::encode(prefix))
}

/// Read `CurrentEra` at the given AssetHub block.
pub async fn planning_era_at_ah_block(ah_rpc: &RpcClient, ah_block: u32) -> Result<u32> {
    let ah_hash: Hash = ah_rpc.get_block_hash(Some(ah_block)).await?;

    let key = plain_key_hex("Staking", "CurrentEra");

    let val: Option<u32> = ah_rpc
        .get_storage_decoded::<u32>(&key, Some(ah_hash))
        .await?;

    val.ok_or_else(|| anyhow!("CurrentEra not found at AH block {}", ah_block))
}
