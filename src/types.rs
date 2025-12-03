// src/types.rs
use serde::{Deserialize, Serialize};
use serde_json;

/// 32-byte block hash.
pub type Hash = [u8; 32];

/// 32-byte account id (`AccountId32` on chain).
pub type AccountId = [u8; 32];

/// Runtime balance type.
pub type Balance = u128;

/// Vote weight type used by EPMB snapshots (`u64`).
pub type VoteWeight = u64;

/// Single voter entry as exposed in the multi-block election snapshot:
///
/// `(who, weight, targets)`
///
/// Mirrors the type:
///   `(AccountId, VoteWeight, BoundedVec<AccountId, MaxVotesPerVoter>)`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VoterSnapshot {
    pub who: AccountId,
    pub weight: VoteWeight,
    pub targets: Vec<AccountId>,
}

/// High-level mirror of what the miner sees via `Snapshot::<T>`:
///
/// - `all_targets`  ≈ `Snapshot::<T>::targets()`
/// - `voter_pages`  ≈ `Snapshot::<T>::voters(page)` for `page in [0 .. Pages)`
/// - `desired_targets` ≈ `Snapshot::<T>::desired_targets()`
///
/// This is the structure consumed by the offline election.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ElectionSnapshot {
    /// Block hash at which the snapshot was read.
    pub at: Hash,
    /// Multi-block election round index.
    pub round: u32,
    /// Total issuance at that block (for debugging / sanity checks).
    pub total_issuance: Balance,
    /// Desired number of winners for this round.
    pub desired_targets: u32,
    /// All candidate targets considered by the election.
    pub all_targets: Vec<AccountId>,
    /// Paged voters, exactly as exposed by the EPMB snapshot (per-page).
    pub voter_pages: Vec<Vec<VoterSnapshot>>,
}

/// Result of an offline election simplified for inspection.
/// Support is in weight units, not raw on-chain balances.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfflineWinner {
    pub validator: AccountId,
    pub support: VoteWeight,
    pub backers: Vec<OfflineBacker>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OfflineBacker {
    pub who: AccountId,
    pub weight: VoteWeight,
}

/// Serialize an `ElectionSnapshot` to pretty JSON.
pub fn snapshot_to_json(snapshot: &ElectionSnapshot) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(snapshot)
}

/// Deserialize an `ElectionSnapshot` from JSON.
pub fn snapshot_from_json(data: &str) -> Result<ElectionSnapshot, serde_json::Error> {
    serde_json::from_str(data)
}

/// Mirror how `SaturatingCurrencyToVote` maps `Balance` (`u128`) -> `VoteWeight` (`u64`):
/// saturating cast from `u128` to `u64`.
pub fn balance_to_vote_weight(b: Balance) -> VoteWeight {
    if b > VoteWeight::MAX as u128 {
        VoteWeight::MAX
    } else {
        b as VoteWeight
    }
}
