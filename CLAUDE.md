# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Catalyst is a Rust-based preconfirmation sidecar for the Taiko Stack, implementing validator registration, lookahead submissions/disputes, and L2 block preconfirmation duties. It's a multi-crate Cargo workspace targeting Rust 1.93 (edition 2024).

## Common Commands

```bash
# Build
cargo build --workspace

# Run all tests
cargo test --workspace --verbose

# Run a single test
cargo test -p <crate-name> <test_name>

# Lint (CI enforces -D warnings)
cargo clippy --all-features -- -D warnings

# Format check
cargo fmt --all -- --check

# Format (apply)
cargo fmt --all

# Dependency audit
cargo deny check

# Typo check
typos

# Sort dependencies (required)
cargo sort --workspace --check
```

The pre-push git hook at `.githooks/pre-push` runs all of the above checks automatically. To activate it: `git config core.hooksPath .githooks`.

## Architecture

### Fork-Based Design

The node supports multiple Taiko protocol forks. The main binary (`node/`) reads `ForkInfo` from config and instantiates the appropriate node variant:

- **`pacaya/`** — Pacaya fork: batch-based preconfirmations with `operator`, `batch_manager`, and `verifier`
- **`shasta/`** — Shasta fork: adds `proposal_manager`, `block_advancer`, and `last_safe_l2_block_finder`
- **`permissionless/`** — Suspended; do not modify without checking `permissionless/README.md`

### `common/` Crate

Shared infrastructure used by all forks:

- **`l1/`** — Ethereum L1 integration: RPC provider, Beacon API, smart contract bindings, blob indexer, slot clock
- **`l2/`** — Taiko L2 integration: Engine API client (`engine/`), Taiko driver
- **`batch_builder/`** — Constructs L2 block batches with configurable size/block limits
- **`chain_monitor/`** — Monitors L1/L2 chain events
- **`signer/`** — Key management (ECDSA private key or Web3Signer)
- **`config/`** — Configuration management
- **`metrics/`** — Prometheus metrics server
- **`utils/`** — RPC client, watchdog, cancellation token, retry logic

### Key Data Flow

1. L1 slot clock triggers block production window
2. `chain_monitor` watches for L1 events (batch proposals, validator lookahead)
3. `batch_builder` / `proposal_manager` assembles L2 transactions into batches
4. L2 engine API advances the L2 head
5. Batch/proposal submitted to L1 mempool via `l1/` module

### URC Module

`urc/` is a separate utility (CLI + monitor) for URC contract interactions with a MySQL backend. It has its own `Makefile` for database management.

## Code Quality Rules

Workspace-level Clippy lints (enforced as errors):
- `unsafe_code = "forbid"` — no unsafe blocks
- `unwrap_used = "deny"` — use `?` or explicit error handling
- `cast_possible_truncation`, `cast_sign_loss`, `cast_precision_loss`, etc. — all denied

Release builds have `overflow-checks = true`.

## Key Dependencies

- **Alloy** (v1.7) — Ethereum RPC, signing, types
- **Tokio** (v1.49) — async runtime
- **taiko-mono** — `taiko_protocol`, `taiko_bindings`, `taiko_preconfirmation_driver`, `taiko_rpc` (sourced from git)
- **alethia-reth** — Taiko-modified Reth (sourced from git)

Allowed git sources in `deny.toml`: taiko-mono, reth, alethia-reth, kona. Adding other git dependencies requires updating `deny.toml`.
