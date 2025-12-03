// src/offchain_exposures.rs

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::election::ElectionOutputs;
use crate::types::{AccountId, Balance, ElectionSnapshot};

/// Offline analogue of on-chain `IndividualExposure` in `Balance` units.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeBacker {
    pub who: AccountId,
    pub stake: Balance,
}

/// Offline analogue of on-chain `Exposure<AccountId, Balance>`.
///
/// All amounts are in `Balance` (`u128`), matching `ErasStakersOverview`
/// and `ErasStakersPaged` on AssetHub.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuntimeExposure {
    pub validator: AccountId,
    pub total: Balance,
    pub own: Balance,
    pub others: Vec<RuntimeBacker>,
}

pub type RuntimeExposureMap = BTreeMap<AccountId, RuntimeExposure>;

/// Build runtime-like exposures from canonical `staked_assignments`.
///
/// Mirrors on-chain behavior in:
/// - `EraElectionPlanner::<T>::collect_exposures`
/// - combined with `CurrencyToVote = SaturatingCurrencyToVote`.
///
/// Since `SaturatingCurrencyToVote::to_currency(value, _)` for `Balance = u128`
/// amounts to a saturating conversion, each `share` (ExtendedBalance) is treated
/// as a `Balance` with a saturating cast.
pub fn build_runtime_exposures_from_staked(
    _snapshot: &ElectionSnapshot,
    outputs: &ElectionOutputs,
) -> RuntimeExposureMap {
    let staked = outputs
        .staked_assignments
        .as_ref()
        .expect("build_runtime_exposures_from_staked called without staked_assignments");

    let mut map: RuntimeExposureMap = BTreeMap::new();

    for ass in staked {
        let nominator = ass.who;

        for (validator, share) in &ass.distribution {
            if *share == 0 {
                continue;
            }

            let stake_balance: Balance = *share as u128;

            let entry = map.entry(*validator).or_insert(RuntimeExposure {
                validator: *validator,
                total: 0,
                own: 0,
                others: Vec::new(),
            });

            entry.total = entry.total.saturating_add(stake_balance);

            if nominator == *validator {
                entry.own = entry.own.saturating_add(stake_balance);
            } else {
                entry.others.push(RuntimeBacker {
                    who: nominator,
                    stake: stake_balance,
                });
            }
        }
    }

    map
}
