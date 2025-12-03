# offline-election-tool-rework

A command‑line tool for reproducing the Polkadot election results offline using
Asset Hub's multi‑block election snapshot (pallet‑election‑provider‑multi‑block)
and comparing them with the relay‑chain validator set.

This tool supports:

- Fetching a full voter/target snapshot from Asset Hub at a chosen block.
- Running the NPoS election offline using `seq_phragmen` with or without global
  reduction (`reduce` step).
- Comparing offline winners against the on‑chain relay validator set at a
  specific relay‑chain block.
- Optional debugging of exposure data (ErasStakersPaged, ErasStakersOverview).

---

## Environment configuration

Create a `.env` file with the following structure:

```
ASSET_HUB_WS=wss://assethub-polkadot-rpc.polkadot.io
RELAY_WS=wss://rpc-polkadot.luckyfriday.io
```

`ASSET_HUB_WS` is required unless passed via CLI.  
`RELAY_WS` is only required when using `--compare-block`.

---

## CLI Overview

The binary exposes two subcommands:

```
offline-election-tool-rework fetch-snapshot
offline-election-tool-rework run-offline
```

Each command can override WS endpoints via CLI flags:

- `--ws` for Asset Hub snapshotting
- `--relay-ws` for the relay chain validator set comparisons

---

# 1. FetchSnapshot

Fetches a full multi‑block election snapshot at a given Asset Hub block.

### Usage

```
offline-election-tool-rework fetch-snapshot     --block <block_number>     --out snapshot.json
```

If `--block` is omitted, the tool uses the current best block.

### Output

A JSON file containing:

- All election targets
- All voter pages
- Snapshot metadata: round, total issuance, desired targets, block hash

This file is later consumed by `run-offline`.

---

# 2. RunOffline

Runs the election logic offline using the snapshot from `FetchSnapshot`.

### Usage

```
offline-election-tool-rework run-offline     --input snapshot.json     --reduce <true|false>     --compare-block <relay_block_number_optional>
```

### Flags

#### `--reduce`

This removes redundant edges from without changing the overall backing of any of the elected candidates.

#### `--compare-block <block>`

When provided, the tool retrieves:

- `Session::Validators` from the relay chain at the given block.
- Compares offline winners vs on‑chain winners.
- Prints intersection, missing validators, and rank‑boundary debugging.

This is the most important comparison flag.

#### Exposure‑related flags

These are optional and used only for debugging:

- `--debug-exposures`
- `--exposure-block`
- `--exposure-era`

They fetch and compare on‑chain `ErasStakersPaged` + `ErasStakersOverview` with the
offline exposure reconstruction. These are primarily diagnostic and not required
for validator‑set matching.

---

# Snapshot Timing on Asset Hub

To obtain a stable election snapshot for a given `planning_era`, follow these
steps.

## 1. Identify the planning window for the era

Asset Hub emits:

```
stakingasync.SessionRotated {
    starting_session,
    active_era,
    planned_era
}
```

Locate:

- The first block where `planned_era == planning_era`. Call this block  
  `B_plan_start`.
- The first subsequent block where `planned_era == planning_era + 1`.  
  Call this block `B_plan_next`.

The interval:

```
[B_plan_start, B_plan_next)
```

is the election planning window for that era.

During this window:

- The chain is planning `planning_era`.
- The election input (voters, targets, desired validators) is finalized and
  remains stable.

## 2. Locate the election execution

Within the planning window, find the first occurrence of:

```
PagedElectionProceeded { page, result }
```

The block containing this event marks:

```
B_elect_start
```

At this point, the voter/target snapshot for `planning_era` is fully frozen and
will not change.

## 3. Choose a snapshot block

Pick any block:

```
M such that B_elect_start ≤ M < B_plan_next
```

At block `M`:

- All snapshot pages are finalized.
- No further voter/target changes occur before the next planning era.

Use this block number when running `FetchSnapshot`.

---

# Finding the matching relay‑chain block for comparison

After determining the election's planning era `E` and its planning window on
Asset Hub:

1. Search Asset Hub events (`stakingasync.SessionRotated`) for the first event
   where:

   ```
   active_era == planned_era == E
   ```

   Extract:

   ```
   S := starting_session
   ```

2. On the relay chain, search for:

   ```
   Session.NewSession { session_index == S }
   ```

   The block containing this event determines the correct relay block matching
   the validator set produced from the snapshot for era **E**.

3. Use that relay block number with:
   ```
   --compare-block <block>
   ```

This aligns the offline election results with the exact validator set active on
the relay chain for era `E`.

---

# Example Workflow

1. **Find planning window and election start**  
   Using Subscan or logs, determine a valid snapshot block `M`.

2. **Fetch snapshot**

   ```
   offline-election-tool-rework fetch-snapshot        --block M        --out era_E_snapshot.json
   ```

3. **Find matching relay‑chain block**  
   Use the procedure above to obtain `relay_block`.

4. **Run offline election and compare**
   ```
   offline-election-tool-rework run-offline        --input era_E_snapshot.json        --compare-block relay_block        --reduce true
   ```

This will print:

- Offline winners
- Match statistics vs relay chain
- Optional debugging when enabled

---

# License

MIT
