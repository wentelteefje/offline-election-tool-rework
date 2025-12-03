// src/ah_multi_block_source.rs

use anyhow::{Context, Result, anyhow};
use subxt::utils::H256;
use subxt::{OnlineClient, config::PolkadotConfig};

use crate::asset_hub;
use crate::types::{AccountId, Balance, ElectionSnapshot, Hash, VoteWeight, VoterSnapshot};

use subxt::config::substrate::AccountId32;

/// Convert runtime `AccountId32` into local `[u8; 32]` alias.
fn account32_to_local(acc: AccountId32) -> AccountId {
    *acc.as_ref()
}

/// Using `pallet-election-provider-multi-block` on AssetHub.
pub struct AhMultiBlockSource {
    client: OnlineClient<PolkadotConfig>,
}

impl AhMultiBlockSource {
    /// Connect to an AssetHub node.
    pub async fn connect(url: &str) -> Result<Self> {
        let client = OnlineClient::<PolkadotConfig>::from_url(url)
            .await
            .context("failed to connect to AssetHub")?;
        Ok(Self { client })
    }

    /// Ensure that the election phase is one where the snapshot is complete and stable.
    ///
    /// Allowed phases:
    ///   Emergency | Signed(_) | SignedValidation(_) | Unsigned(_) | Export(_) | Done
    async fn ensure_phase_allows_snapshot(&self, at_hash: H256) -> Result<()> {
        use crate::asset_hub::api::runtime_types::pallet_election_provider_multi_block::types::Phase;

        let storage = self.client.storage().at(at_hash);
        let root_storage = asset_hub::api::storage();
        let epmb = root_storage.multi_block_election();

        let phase_addr = epmb.current_phase();
        let phase: Phase = storage
            .fetch(&phase_addr)
            .await?
            .context("CurrentPhase storage returned None at this block")?;

        let allowed = matches!(
            phase,
            Phase::Emergency
                | Phase::Signed(_)
                | Phase::SignedValidation(_)
                | Phase::Unsigned(_)
                | Phase::Export(_)
                | Phase::Done
        );

        if !allowed {
            return Err(anyhow!(
                "snapshot is not guaranteed to be complete in current phase: {:?}",
                phase
            ));
        }

        Ok(())
    }

    /// Build an `ElectionSnapshot` from pallet-election-provider-multi-block
    /// for the current round at the given block.
    ///
    /// - `at` is the `[u8; 32]` block hash.
    /// - `max_pages` is the runtime `MultiBlockElection::Pages` value or a safe upper bound.
    pub async fn snapshot_at(&self, at: Hash, max_pages: u32) -> Result<ElectionSnapshot> {
        let at_hash = H256::from(at);

        // Ensure a stable snapshot phase.
        self.ensure_phase_allows_snapshot(at_hash).await?;

        let storage = self.client.storage().at(at_hash);
        let root_storage = asset_hub::api::storage();

        let epmb = root_storage.multi_block_election();
        let balances = root_storage.balances();

        // Read the current round: Round<T> = u32.
        let round_addr = epmb.round();
        let round: u32 = storage
            .fetch(&round_addr)
            .await?
            .context("Round storage returned None at this block")?;

        // Rebuild `all_targets` from paged target snapshots.
        //
        // Storage type:
        //   PagedTargetSnapshot(round, page) :
        //     BoundedVec<AccountId32>
        //
        // Scan [0..max_pages) and concatenate all pages in order, mirroring
        // how the internal snapshot helper flattens targets.
        use std::collections::BTreeSet;
        let mut target_set: BTreeSet<AccountId> = BTreeSet::new();
        let mut all_targets: Vec<AccountId> = Vec::new();

        for page_idx in 0..max_pages {
            let t_addr = epmb.paged_target_snapshot(round, page_idx);
            let page_opt = storage.fetch(&t_addr).await?;

            let Some(targets_page) = page_opt else {
                continue;
            };

            // BoundedVec<T> is a tuple struct, so `.0` accesses the inner Vec.
            for acc in targets_page.0 {
                let raw: AccountId = account32_to_local(acc);
                if target_set.insert(raw) {
                    all_targets.push(raw);
                }
            }
        }

        // Rebuild per-page voters from `PagedVoterSnapshot(round, page)`.
        //
        // Storage type:
        //   PagedVoterSnapshot(round, page) :
        //     BoundedVec<(AccountId32, u64, BoundedVec<AccountId32>)>
        //
        // This corresponds 1:1 to `VoterOf<MinerConfig>`.
        let mut voter_pages: Vec<Vec<VoterSnapshot>> = Vec::with_capacity(max_pages as usize);

        for page_idx in 0..max_pages {
            let v_addr = epmb.paged_voter_snapshot(round, page_idx);
            let page_opt = storage.fetch(&v_addr).await?;

            let mut this_page: Vec<VoterSnapshot> = Vec::new();

            if let Some(voters_page) = page_opt {
                for (who, weight, targets) in voters_page.0 {
                    let stash: AccountId = account32_to_local(who);

                    let mapped_targets: Vec<AccountId> =
                        targets.0.into_iter().map(account32_to_local).collect();

                    this_page.push(VoterSnapshot {
                        who: stash,
                        weight: weight as VoteWeight,
                        targets: mapped_targets,
                    });
                }
            }

            voter_pages.push(this_page);
        }

        // Total issuance at that block (Balances::TotalIssuance).
        let total_issuance_addr = balances.total_issuance();
        let total_issuance: Balance = storage.fetch(&total_issuance_addr).await?.unwrap_or(0);

        // DesiredTargets(round) = desired validator count for this round.
        let desired_addr = epmb.desired_targets(round);
        let desired: Option<u32> = storage.fetch(&desired_addr).await?;
        let desired_targets = desired.unwrap_or(all_targets.len() as u32);

        Ok(ElectionSnapshot {
            at,
            round,
            total_issuance,
            desired_targets,
            all_targets,
            voter_pages,
        })
    }
}
