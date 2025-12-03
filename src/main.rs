// src/main.rs
mod ah_multi_block_source;
mod asset_hub;
mod compare;
mod election;
mod nominator_debug;
mod offchain_exposures;
mod onchain_exposures;
mod rpc;
mod storage_keys;
mod types;

use crate::ah_multi_block_source::AhMultiBlockSource;
use crate::compare::{compare_with_relay, debug_boundary_ranks, fetch_relay_session_validators};
use crate::election::{
    run_offline_election_with_stake, staked_assignments_to_offline_winners,
    verify_staked_assignments_internal,
};
use crate::nominator_debug::{build_offline_nom_view, build_onchain_nom_view, debug_nominator};
use crate::offchain_exposures::build_runtime_exposures_from_staked;
use crate::onchain_exposures::{
    fetch_active_era_at, fetch_current_era_at, fetch_onchain_exposures_for_era,
    fetch_overviews_for_validators, flatten_onchain_backers,
};
use crate::rpc::RpcClient;
use crate::storage_keys::planning_era_at_ah_block;
use crate::types::{AccountId, Balance, Hash, snapshot_from_json, snapshot_to_json};

use subxt::{OnlineClient, config::PolkadotConfig};

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

/// Upper bound for number of pages in EPMB snapshots.
/// AssetHub typically uses few pages; [0..MAX_PAGES) is scanned.
const MAX_PAGES: u32 = 32;

#[derive(Parser)]
#[command(name = "offline-election-ah", version)]
struct Cli {
    /// WS endpoint of Asset Hub node.
    ///
    /// If not provided, the value from `ASSET_HUB_WS` is used.
    #[arg(global = true, long)]
    ws: Option<String>,

    /// WS endpoint of the relay chain node (for validator set comparison).
    ///
    /// If not provided, the value from `RELAY_WS` is used when needed.
    #[arg(global = true, long)]
    relay_ws: Option<String>,

    #[command(subcommand)]
    cmd: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch on-chain data at a block and save multi-block snapshot as JSON.
    FetchSnapshot {
        /// Block number on AssetHub; omit for best block.
        #[arg(long)]
        block: Option<u32>,

        /// Output JSON file.
        #[arg(long)]
        out: PathBuf,
    },

    /// Run offline election from a previously saved snapshot JSON.
    RunOffline {
        /// Snapshot JSON file.
        #[arg(long)]
        input: PathBuf,

        /// Optional relay block number to compare with on-chain validator set.
        #[arg(long)]
        compare_block: Option<u32>,

        /// Enable exposure and nominator distribution debugging.
        #[arg(long)]
        debug_exposures: bool,

        /// AssetHub block number used when fetching on-chain exposures
        /// (required when `--debug-exposures` is set).
        #[arg(long)]
        exposure_block: Option<u32>,

        /// Era index used when fetching on-chain exposures (exposure_block needs to be fresh enough to have the data corresponding to the Era)
        /// (required when `--debug-exposures` is set).
        #[arg(long)]
        exposure_era: Option<u32>,

        /// Whether to run the offline election with global reduction (`reduce` step).
        /// Defaults to `true`.
        #[arg(long, default_value_t = true)]
        reduce: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables from `.env` if present.
    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    // Resolve AssetHub WS endpoint:
    //   1. CLI `--ws`
    //   2. `ASSET_HUB_WS` env var
    let ws = cli.ws.clone().unwrap_or_else(|| {
        std::env::var("ASSET_HUB_WS").expect("ASSET_HUB_WS must be set or --ws provided")
    });

    // Resolve relay WS endpoint:
    //   1. CLI `--relay-ws`
    //   2. `RELAY_WS` env var (optional; only required when comparison is used)
    let relay_ws: Option<String> = cli
        .relay_ws
        .clone()
        .or_else(|| std::env::var("RELAY_WS").ok());

    match cli.cmd {
        Commands::FetchSnapshot { block, out } => {
            // Resolve block number → hash on AssetHub.
            let rpc_client = RpcClient::connect(&ws).await?;
            let at: Hash = rpc_client.get_block_hash(block).await?;
            eprintln!("Using AssetHub block hash 0x{}", hex::encode(at));

            // Determine planning era at the snapshot block on AssetHub.
            if let Some(block_number) = block {
                let planning_era = planning_era_at_ah_block(&rpc_client, block_number).await?;
                println!(
                    "[info] AH block {} has planning era (CurrentEra) = {}",
                    block_number, planning_era
                );
            } else {
                println!("[info] AH block is best block; planning era not resolved by number");
            }

            // Use the Subxt-based multi-block source to pull the snapshot at that hash.
            let source = AhMultiBlockSource::connect(&ws).await?;
            let snapshot = source.snapshot_at(at, MAX_PAGES).await?;

            // Serialize snapshot to JSON.
            let json = snapshot_to_json(&snapshot)?;
            fs::write(&out, json)?;
            eprintln!("Snapshot written to {}", out.display());
        }

        Commands::RunOffline {
            input,
            compare_block,
            debug_exposures,
            exposure_block,
            exposure_era,
            reduce,
        } => {
            // Load snapshot from JSON.
            let data = fs::read_to_string(&input)?;
            let snapshot = snapshot_from_json(&data)?;

            // Run offline election with stake pipeline, controlled by `--reduce`.
            let outputs = run_offline_election_with_stake(&snapshot, reduce)?;
            let res = &outputs.raw;
            let winners = staked_assignments_to_offline_winners(&outputs);

            eprintln!("Offline winners ({}):", winners.len());
            for (i, w) in winners.iter().enumerate() {
                eprintln!(
                    "#{:<4} validator=0x{} support={} backers={}",
                    i,
                    hex::encode(w.validator),
                    w.support,
                    w.backers.len()
                );
            }

            if let Err(e) = verify_staked_assignments_internal(&snapshot, &outputs) {
                eprintln!("WARNING: internal stake verification failed: {e:?}");
            }

            // Optional: debug exposures and nominator distributions.
            if debug_exposures {
                let exposure_block = match exposure_block {
                    Some(b) => b,
                    None => {
                        return Err(anyhow::anyhow!(
                            "--debug-exposures requires --exposure-block <block_number>"
                        ));
                    }
                };

                let exposure_era = match exposure_era {
                    Some(e) => e,
                    None => {
                        return Err(anyhow::anyhow!(
                            "--debug-exposures requires --exposure-era <era_index>"
                        ));
                    }
                };

                // Build global snapshot voter set for debugging.
                let mut all_snapshot_voters: BTreeSet<AccountId> = BTreeSet::new();
                for page in &snapshot.voter_pages {
                    for v in page {
                        all_snapshot_voters.insert(v.who);
                    }
                }

                // Build runtime-like exposures (per validator: total, own, nominators)
                // in `Balance` units, using the same pipeline as on-chain.
                let offline_exposures = build_runtime_exposures_from_staked(&snapshot, &outputs);

                // Connect a Subxt client to AssetHub.
                let ah_client = OnlineClient::<PolkadotConfig>::from_url(&ws).await?;

                // Resolve exposure block number to block hash.
                let ah_rpc = RpcClient::connect(&ws).await?;
                let at_ah: Hash = ah_rpc.get_block_hash(Some(exposure_block)).await?;

                // Fetch `CurrentEra` and `ActiveEra` at the exposure block, for information.
                let current_era_on_chain = fetch_current_era_at(&ah_client, at_ah).await?;
                let active_era_on_chain = fetch_active_era_at(&ah_client, at_ah).await?;

                println!(
                    "[info] on-chain CurrentEra={} ActiveEra={} at exposure block (user-requested era={})",
                    current_era_on_chain, active_era_on_chain, exposure_era,
                );

                eprintln!(
                    "[info] Using AssetHub block hash for exposure comparison: 0x{}",
                    hex::encode(at_ah)
                );

                // Collect validator set from offline winners.
                let offline_validators: Vec<AccountId> =
                    winners.iter().map(|w| w.validator).collect();

                // Fetch paged exposures and overview metadata from on-chain
                // for the user-specified era.
                let onchain_pages = fetch_onchain_exposures_for_era(
                    &ah_client,
                    at_ah,
                    exposure_era,
                    &offline_validators,
                    MAX_PAGES,
                )
                .await?;

                let onchain_overviews = fetch_overviews_for_validators(
                    &ah_client,
                    at_ah,
                    exposure_era,
                    &offline_validators,
                )
                .await?;

                // Flatten paged on-chain exposures into `validator -> {nominator -> stake}`.
                let onchain_flat = flatten_onchain_backers(&onchain_pages);

                let offline_nom_view = build_offline_nom_view(&offline_exposures);
                let onchain_nom_view = build_onchain_nom_view(&onchain_flat);

                // Compare per-validator nominator sets and counts.
                let mut matched_nominator_sets = 0usize;
                let mut mismatched_nominator_sets = 0usize;

                // Limit how many validators are debugged in detail.
                let mut debug_mismatches_left = 5usize;

                for (validator, off_exp) in &offline_exposures {
                    // Offline nominators for this validator (set of AccountId).
                    let off_nom_set: BTreeSet<AccountId> =
                        off_exp.others.iter().map(|b| b.who).collect();

                    // On-chain nominators for this validator: map nominator -> stake (Balance).
                    let on_nom_map = onchain_flat
                        .get(validator)
                        .cloned()
                        .unwrap_or_else(BTreeMap::new);

                    let on_nom_set: BTreeSet<AccountId> = on_nom_map.keys().copied().collect();

                    // On-chain overview (total, own, counts).
                    let on_overview = match onchain_overviews.get(validator) {
                        Some(ov) => ov,
                        None => {
                            eprintln!(
                                "[warn] No on-chain ErasStakersOverview for validator 0x{} in era {}",
                                hex::encode(validator),
                                exposure_era,
                            );
                            mismatched_nominator_sets += 1;
                            continue;
                        }
                    };

                    let off_count = off_nom_set.len();
                    let on_count = on_nom_set.len();

                    // Check that the number of nominators matches the on-chain metadata.
                    if on_count as u32 != on_overview.nominator_count {
                        eprintln!(
                            "[warn] Validator 0x{}: on-chain nominator_count={} but flattened pages have {} nominators",
                            hex::encode(validator),
                            on_overview.nominator_count,
                            on_count,
                        );
                    }

                    // Core set equality check.
                    if off_nom_set == on_nom_set {
                        matched_nominator_sets += 1;
                    } else {
                        mismatched_nominator_sets += 1;

                        let only_offline: Vec<_> =
                            off_nom_set.difference(&on_nom_set).copied().collect();
                        let only_onchain: Vec<_> =
                            on_nom_set.difference(&off_nom_set).copied().collect();

                        eprintln!(
                            "[mismatch] Validator 0x{}: nominator sets differ. only_offline={} only_onchain={}",
                            hex::encode(validator),
                            only_offline.len(),
                            only_onchain.len(),
                        );

                        if debug_mismatches_left > 0 {
                            debug_mismatches_left -= 1;

                            // Count how many on-chain-only nominators are present in the snapshot.
                            let mut only_onchain_in_snapshot = 0usize;
                            let mut only_onchain_not_in_snapshot = 0usize;

                            for who in &only_onchain {
                                if all_snapshot_voters.contains(who) {
                                    only_onchain_in_snapshot += 1;
                                } else {
                                    only_onchain_not_in_snapshot += 1;
                                }
                            }

                            eprintln!(
                                "    only_onchain_in_snapshot={} only_onchain_not_in_snapshot={}",
                                only_onchain_in_snapshot, only_onchain_not_in_snapshot,
                            );

                            // Example nominators unique to offline.
                            if !only_offline.is_empty() {
                                eprintln!(
                                    "    nominators only in OFFLINE assignment for this validator (first 5):"
                                );
                                for who in only_offline.iter().take(5) {
                                    eprintln!("      OFF  0x{}", hex::encode(who));
                                }
                            }

                            // Example nominators unique to on-chain.
                            if !only_onchain.is_empty() {
                                eprintln!(
                                    "    nominators only in ON-CHAIN exposure for this validator (first 5):"
                                );
                                for who in only_onchain.iter().take(5) {
                                    eprintln!("      ON   0x{}", hex::encode(who));
                                }
                            }

                            // Compare stakes for nominators that are present in both sets.
                            let mut stake_mismatches = 0usize;
                            eprintln!("    common nominators with stake differences (first 10):");

                            for who in off_nom_set.intersection(&on_nom_set).take(50) {
                                // Offline stake in Balance.
                                let off_stake: Balance = off_exp
                                    .others
                                    .iter()
                                    .find(|b| b.who == *who)
                                    .map(|b| b.stake)
                                    .unwrap_or(0);

                                // On-chain stake in Balance.
                                let on_stake: Balance = *on_nom_map.get(who).unwrap_or(&0u128);

                                if off_stake != on_stake {
                                    stake_mismatches += 1;
                                    if stake_mismatches <= 10 {
                                        let off_vote =
                                            crate::types::balance_to_vote_weight(off_stake);
                                        let on_vote =
                                            crate::types::balance_to_vote_weight(on_stake);

                                        eprintln!(
                                            "      0x{}: off_stake={} on_stake={} off_vote={} on_vote={}",
                                            hex::encode(who),
                                            off_stake,
                                            on_stake,
                                            off_vote,
                                            on_vote,
                                        );
                                    }
                                }
                            }

                            eprintln!(
                                "    common nominators with differing stakes: {} (capped at 10 shown above)",
                                stake_mismatches,
                            );

                            // Pick a few nominators from each side for detailed debugging.
                            for who in only_onchain.iter().take(2) {
                                eprintln!("  --- DEBUG nominator only_onchain ---");
                                debug_nominator(who, &offline_nom_view, &onchain_nom_view);
                            }

                            for who in only_offline.iter().take(2) {
                                eprintln!("  --- DEBUG nominator only_offline ---");
                                debug_nominator(who, &offline_nom_view, &onchain_nom_view);
                            }
                        }
                    }

                    // Aggregate totals: compare `Balance` and vote-space views.
                    let off_total = off_exp.total;
                    let off_own = off_exp.own;

                    let on_total = on_overview.total;
                    let on_own = on_overview.own;

                    let off_total_as_vote = crate::types::balance_to_vote_weight(off_total);
                    let off_own_as_vote = crate::types::balance_to_vote_weight(off_own);
                    let on_total_as_vote = crate::types::balance_to_vote_weight(on_total);
                    let on_own_as_vote = crate::types::balance_to_vote_weight(on_own);

                    eprintln!(
                        "[exposure] validator=0x{} \
                        off_total={} off_own={} \
                        on_total={} on_own={} \
                        off_total_as_vote={} off_own_as_vote={} \
                        on_total_as_vote={} on_own_as_vote={} \
                        nominators_offline={} nominators_onchain={}",
                        hex::encode(validator),
                        off_total,
                        off_own,
                        on_total,
                        on_own,
                        off_total_as_vote,
                        off_own_as_vote,
                        on_total_as_vote,
                        on_own_as_vote,
                        off_count,
                        on_count,
                    );
                }

                eprintln!(
                    "[summary] exposure comparison vs AssetHub era {}: matched_nominator_sets={} mismatched_nominator_sets={}",
                    exposure_era, matched_nominator_sets, mismatched_nominator_sets,
                );
            }

            // Optional: compare with relay `Session::Validators` at a given block.
            if let Some(block) = compare_block {
                if let Some(relay_ws) = &relay_ws {
                    let relay_client = RpcClient::connect(relay_ws).await?;
                    let at_relay: Hash = relay_client.get_block_hash(Some(block)).await?;

                    let onchain = fetch_relay_session_validators(&relay_client, at_relay).await?;
                    eprintln!(
                        "On-chain RELAY Session::Validators at block {}: {} entries",
                        block,
                        onchain.len()
                    );

                    // Detailed diff and boundary debugging.
                    compare_with_relay(&snapshot, &res, &onchain);
                    debug_boundary_ranks(&winners, &onchain);
                } else {
                    eprintln!(
                        "WARNING: --compare-block was given but --relay-ws/RELAY_WS is missing; \
                         cannot compare against relay Session::Validators."
                    );
                }
            }
        }
    }

    Ok(())
}
