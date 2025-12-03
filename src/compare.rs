// src/compare.rs
use crate::election::RawElectionResult;
use crate::rpc::RpcClient;
use crate::storage_keys::plain_key_hex;
use crate::types::{AccountId, ElectionSnapshot, Hash, OfflineWinner};
use anyhow::{Result, anyhow};
use parity_scale_codec::Decode;
use std::collections::{BTreeSet, HashMap};

/// Fetch validator set from relay chain `Session::Validators` at a given block.
pub async fn fetch_relay_session_validators(
    client: &RpcClient,
    at: Hash,
) -> Result<Vec<AccountId>> {
    let key = plain_key_hex("Session", "Validators");

    if let Some(bytes) = client.get_storage(&key, Some(at)).await? {
        let mut slice = &bytes[..];
        let vals: Vec<AccountId> = Decode::decode(&mut slice)
            .map_err(|e| anyhow!("decode Session::Validators: {:?}", e))?;
        Ok(vals)
    } else {
        eprintln!(
            "WARN: Session::Validators returned None at block 0x{}",
            hex::encode(at)
        );
        Ok(Vec::new())
    }
}

/// Compare two validator sets and return:
/// (intersection, only_offline, only_onchain).
pub fn compare_winners_with_chain(
    offline: &[AccountId],
    onchain: &[AccountId],
) -> (usize, usize, usize) {
    use std::collections::HashSet;
    let off: HashSet<_> = offline.iter().copied().collect();
    let on: HashSet<_> = onchain.iter().copied().collect();

    let intersection = off.intersection(&on).count();
    let only_offline = off.difference(&on).count();
    let only_onchain = on.difference(&off).count();

    (intersection, only_offline, only_onchain)
}

/// Format `AccountId` as hex string.
pub fn fmt_account(id: &AccountId) -> String {
    format!("0x{}", hex::encode(id))
}

pub fn compare_with_relay(
    snapshot: &ElectionSnapshot,
    res: &RawElectionResult,
    onchain_validators: &[AccountId],
) {
    let offline_winners: Vec<&AccountId> = res.winners.iter().map(|(v, _)| v).collect();

    let offline_set: BTreeSet<AccountId> = offline_winners.iter().cloned().cloned().collect();
    let onchain_set: BTreeSet<AccountId> = onchain_validators.iter().cloned().collect();

    let only_offline: Vec<AccountId> = offline_set.difference(&onchain_set).cloned().collect();
    let only_onchain: Vec<AccountId> = onchain_set.difference(&offline_set).cloned().collect();

    let match_count = offline_set.len() - only_offline.len();

    println!(
        "Comparison with RELAY Session::Validators: match={}, only_offline={}, only_onchain={}",
        match_count,
        only_offline.len(),
        only_onchain.len(),
    );

    // Detailed diff.

    if !only_offline.is_empty() {
        println!("\nValidators only in OFFLINE winners (not on-chain):");
        for id in &only_offline {
            if let Some((idx, (_val, support))) =
                res.winners.iter().enumerate().find(|(_, (v, _))| v == id)
            {
                println!("  rank #{:<3} {} support={}", idx, fmt_account(id), support,);
            } else {
                println!("  {}", fmt_account(id));
            }
        }
    }

    if !only_onchain.is_empty() {
        println!("\nValidators only in ON-CHAIN winners (not offline):");
        for id in &only_onchain {
            let in_snapshot = snapshot.all_targets.iter().any(|val| val == id);

            println!(
                "  {} (in snapshot.all_targets: {})",
                fmt_account(id),
                if in_snapshot { "yes" } else { "NO" },
            );
        }
    }
}

/// Debug helper for validators that differ between offline and on-chain results.
///
/// - `offline_winners` is the sorted offline winner list.
/// - `onchain_validators` is the `Session::Validators` list from the relay chain.
pub fn debug_boundary_ranks(offline_winners: &[OfflineWinner], onchain_validators: &[AccountId]) {
    // Map: validator -> (rank, support).
    let mut rank_map: HashMap<AccountId, (usize, u128)> = HashMap::new();
    for (idx, w) in offline_winners.iter().enumerate() {
        rank_map.insert(w.validator, (idx, w.support as u128));
    }

    let offline_set: BTreeSet<AccountId> = offline_winners.iter().map(|w| w.validator).collect();
    let onchain_set: BTreeSet<AccountId> = onchain_validators.iter().copied().collect();

    let only_offline: Vec<AccountId> = offline_set.difference(&onchain_set).copied().collect();
    let only_onchain: Vec<AccountId> = onchain_set.difference(&offline_set).copied().collect();

    eprintln!(
        "BOUNDARY DEBUG: only_offline = {}, only_onchain = {}",
        only_offline.len(),
        only_onchain.len()
    );

    // Details for validators that are only in offline winners.
    for v in &only_offline {
        if let Some((rank, support)) = rank_map.get(v) {
            eprintln!(
                "  OFFLINE-ONLY 0x{} at offline rank {} with support {}",
                hex::encode(v),
                rank,
                support
            );

            let start = rank.saturating_sub(3);
            let end = usize::min(rank + 3, offline_winners.len().saturating_sub(1));

            eprintln!("    Neighbours around that rank:");
            for i in start..=end {
                let w = &offline_winners[i];
                eprintln!(
                    "      {} rank {:4} 0x{} support={}",
                    if i == *rank { ">>" } else { "  " },
                    i,
                    hex::encode(w.validator),
                    w.support
                );
            }
        } else {
            eprintln!(
                "  OFFLINE-ONLY 0x{} but not found in rank_map (unexpected)",
                hex::encode(v)
            );
        }
    }

    // Validators that are only present on-chain.
    for v in &only_onchain {
        eprintln!(
            "  ONCHAIN-ONLY 0x{} did not appear in offline winners",
            hex::encode(v)
        );
    }
}
