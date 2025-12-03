// src/election.rs

use crate::types::{
    AccountId, ElectionSnapshot, OfflineBacker, OfflineWinner, VoteWeight, VoterSnapshot,
};

use anyhow::Result;
use sp_arithmetic::PerU16;
use sp_npos_elections::{
    ElectionResult, StakedAssignment, assignment_ratio_to_staked_normalized,
    assignment_staked_to_ratio_normalized, reduce, seq_phragmen,
};
use std::collections::HashMap;

/// Raw output of `sp_npos_elections::seq_phragmen`.
pub type RawElectionResult = ElectionResult<AccountId, PerU16>;

/// Flatten `voter_pages` into a single vector, matching `BaseMiner::mine_solution`.
fn flatten_voters(snapshot: &ElectionSnapshot) -> Vec<(AccountId, VoteWeight, Vec<AccountId>)> {
    snapshot
        .voter_pages
        .iter()
        .flat_map(|page: &Vec<VoterSnapshot>| page.iter())
        .map(|v| (v.who, v.weight, v.targets.clone()))
        .collect()
}

/// Canonical election outputs:
/// - `raw`: winners and ratio assignments (`PerU16`).
/// - `staked_assignments`: same assignments in `VoteWeight` units,
pub struct ElectionOutputs {
    pub raw: RawElectionResult,
    pub staked_assignments: Option<Vec<StakedAssignment<AccountId>>>,
}

/// Run `seq_phragmen` and additionally compute canonical staked assignments.
pub fn run_offline_election_with_stake(
    snapshot: &ElectionSnapshot,
    do_reduce: bool,
) -> Result<ElectionOutputs> {
    // Flatten voters and clone targets.
    let all_targets: Vec<AccountId> = snapshot.all_targets.clone();
    let all_voters: Vec<(AccountId, VoteWeight, Vec<AccountId>)> = flatten_voters(snapshot);
    let to_elect = snapshot.desired_targets as usize;

    // Run seq_phragmen.
    let ElectionResult {
        winners,
        assignments,
    } = seq_phragmen::<AccountId, PerU16>(to_elect, all_targets.clone(), all_voters.clone(), None)
        .map_err(|e| anyhow::anyhow!("seq_phragmen failed: {:?}", e))?;

    // Build `stake_of` from the flattened voter list using `VoteWeight` (u64).
    let mut stake_map: HashMap<AccountId, VoteWeight> = HashMap::new();
    for (who, weight, _) in &all_voters {
        stake_map.insert(*who, *weight);
    }

    let stake_of = move |who: &AccountId| -> VoteWeight { *stake_map.get(who).unwrap_or(&0u64) };

    // Convert ratio assignments -> staked assignments (canonical helper).
    let mut staked: Vec<StakedAssignment<AccountId>> =
        assignment_ratio_to_staked_normalized(assignments, &stake_of).map_err(|e| {
            anyhow::anyhow!("assignment_ratio_to_staked_normalized failed: {:?}", e)
        })?;

    // Optional global reduction, matching miner behavior.
    if do_reduce {
        let _reduced_edges = reduce(&mut staked);
    }

    // Convert staked assignments back to ratio space (as in `BaseMiner`).
    let final_ratio_assignments = assignment_staked_to_ratio_normalized(staked.clone())
        .map_err(|e| anyhow::anyhow!("assignment_staked_to_ratio_normalized failed: {:?}", e))?;

    Ok(ElectionOutputs {
        raw: RawElectionResult {
            winners,
            assignments: final_ratio_assignments,
        },
        staked_assignments: Some(staked),
    })
}

/// Build `OfflineWinner` list from canonical staked assignments.
///
/// Uses the output of `run_offline_election_with_stake`:
/// - `support` is the sum of stake shares in `VoteWeight` units.
/// - `backers` is the distribution of those stake shares.
/// - winners are ordered by their election rank (`raw.winners` order).
pub fn staked_assignments_to_offline_winners(outputs: &ElectionOutputs) -> Vec<OfflineWinner> {
    let staked = outputs
        .staked_assignments
        .as_ref()
        .expect("staked_assignments_to_offline_winners called without staked_assignments");

    use std::collections::HashMap;

    // Aggregate by validator.
    let mut by_validator: HashMap<AccountId, OfflineWinner> = HashMap::new();

    for assignment in staked {
        let nominator = assignment.who;
        for (validator, share) in &assignment.distribution {
            let entry = by_validator
                .entry(*validator)
                .or_insert_with(|| OfflineWinner {
                    validator: *validator,
                    support: 0,
                    backers: Vec::new(),
                });

            // Election weights are < total issuance < 2^64, so this cast is safe.
            let share_u64 = (*share as u128).min(u64::MAX as u128) as u64;
            entry.support = entry.support.saturating_add(share_u64);
            entry.backers.push(OfflineBacker {
                who: nominator,
                weight: share_u64,
            });
        }
    }

    // Order winners according to `raw.winners` (election rank).
    let mut ordered: Vec<OfflineWinner> = Vec::with_capacity(outputs.raw.winners.len());

    for (validator, _score) in &outputs.raw.winners {
        if let Some(w) = by_validator.remove(validator) {
            ordered.push(w);
        } else {
            ordered.push(OfflineWinner {
                validator: *validator,
                support: 0,
                backers: Vec::new(),
            });
        }
    }

    ordered
}

/// Internal consistency check for staked assignments:
/// - For each nominator, `sum(share)` should be <= `stake_of(nominator)` and
///   typically equal up to rounding.
/// - Sum over all validator supports should match sum of all nominators' stake
///   up to rounding.
pub fn verify_staked_assignments_internal(
    snapshot: &ElectionSnapshot,
    outputs: &ElectionOutputs,
) -> Result<()> {
    let staked = outputs
        .staked_assignments
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No staked_assignments present"))?;

    use std::collections::HashMap;

    // Map: nominator -> stake (from snapshot).
    let mut stake_of: HashMap<AccountId, VoteWeight> = HashMap::new();
    for page in &snapshot.voter_pages {
        for v in page {
            stake_of.insert(v.who, v.weight);
        }
    }

    // Per-nominator totals.
    for ass in staked {
        let nominator = ass.who;
        let expected = *stake_of.get(&nominator).unwrap_or(&0);
        let mut total: VoteWeight = 0;

        for (_validator, share) in &ass.distribution {
            let share_u64 = (*share as u128).min(u64::MAX as u128) as u64;
            total = total.saturating_add(share_u64);
        }

        if total > expected {
            return Err(anyhow::anyhow!(
                "Nominator 0x{} assigned more stake ({}) than they have ({})",
                hex::encode(nominator),
                total,
                expected,
            ));
        }
    }

    // Global totals: sum of supports vs sum of all nominator weights.
    let winners = staked_assignments_to_offline_winners(outputs);
    let mut total_support: VoteWeight = 0;
    for w in &winners {
        total_support = total_support.saturating_add(w.support);
    }

    let total_stake: VoteWeight = stake_of.values().copied().sum();

    if total_support > total_stake {
        return Err(anyhow::anyhow!(
            "Global support ({}) exceeds total stake ({})",
            total_support,
            total_stake,
        ));
    }

    Ok(())
}
