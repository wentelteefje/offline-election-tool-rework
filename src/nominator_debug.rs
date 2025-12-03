// src/nominator_debug.rs

use std::collections::BTreeMap;

use crate::offchain_exposures::RuntimeExposureMap;
use crate::onchain_exposures::OnchainFlattenedExposures;
use crate::types::{AccountId, Balance, balance_to_vote_weight};

/// Nominator-centric view:
/// nominator -> (validator -> stake).
pub type NomView = BTreeMap<AccountId, BTreeMap<AccountId, Balance>>;

/// Build nominator-centric view from offline runtime exposures.
pub fn build_offline_nom_view(off: &RuntimeExposureMap) -> NomView {
    let mut view: NomView = BTreeMap::new();
    for (val, exp) in off {
        for b in &exp.others {
            view.entry(b.who)
                .or_default()
                .entry(*val)
                .and_modify(|s| *s = s.saturating_add(b.stake))
                .or_insert(b.stake);
        }
        if exp.own > 0 {
            view.entry(*val)
                .or_default()
                .entry(*val)
                .and_modify(|s| *s = s.saturating_add(exp.own))
                .or_insert(exp.own);
        }
    }
    view
}

/// Build nominator-centric view from on-chain flattened exposures.
pub fn build_onchain_nom_view(on: &OnchainFlattenedExposures) -> NomView {
    let mut view: NomView = BTreeMap::new();
    for (val, backers) in on {
        for (nom, stake) in backers {
            view.entry(*nom)
                .or_default()
                .entry(*val)
                .and_modify(|s| *s = s.saturating_add(*stake))
                .or_insert(*stake);
        }
    }
    view
}

/// Print detailed per-nominator comparison between offline and on-chain exposures.
pub fn debug_nominator(who: &AccountId, offline_nom_view: &NomView, onchain_nom_view: &NomView) {
    let off = offline_nom_view.get(who);
    let on = onchain_nom_view.get(who);

    eprintln!("NOMINATOR 0x{}", hex::encode(who));

    let mut total_off: Balance = 0;
    let mut total_on: Balance = 0;

    if let Some(map) = off {
        eprintln!("  OFFLINE:");
        for (val, stake) in map {
            total_off = total_off.saturating_add(*stake);
            eprintln!(
                "    -> validator 0x{} stake={} vote={}",
                hex::encode(val),
                stake,
                balance_to_vote_weight(*stake),
            );
        }
    } else {
        eprintln!("  OFFLINE: (no assignments)");
    }

    if let Some(map) = on {
        eprintln!("  ON-CHAIN:");
        for (val, stake) in map {
            total_on = total_on.saturating_add(*stake);
            eprintln!(
                "    -> validator 0x{} stake={} vote={}",
                hex::encode(val),
                stake,
                balance_to_vote_weight(*stake),
            );
        }
    } else {
        eprintln!("  ON-CHAIN: (no assignments)");
    }

    eprintln!(
        "  TOTALS: off_total={} on_total={} off_vote={} on_vote={}",
        total_off,
        total_on,
        balance_to_vote_weight(total_off),
        balance_to_vote_weight(total_on),
    );
}
