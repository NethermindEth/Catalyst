# Changelog

All notable changes to Catalyst are documented here, organized by release version.

## [v1.34.12] — 2026-03-27

### Features
- Support HTTP URLs as the main RPC (#918)

---

## [v1.34.11] — 2026-03-26

### Features
- Switch from `taikoAuth_lastBlockIDByBatchID` to `taikoAuth_lastCertainBlockIDByBatchID` RPC endpoint (#917)
- Skip tx_list compression during recovery mode (#914)
- Treat rehearsal chain ID as mainnet for derivation purposes (#899)
- Shasta: select derivation parameters based on chain ID (#892)
- Proposal builder refactored to build proposals in a separate thread (#898, #910)

### Fixes
- Validate blob size is within limits before submission (#906)
- Apply derivation rules correctly during block recovery (#911)

### Dependencies
- Updated taiko-mono and alethia-reth dependencies (#905)
- Upgraded to Alloy 1.7 (#902)
- Bumped libp2p-gossipsub 0.49.2 → 0.49.3 (#912)
- Bumped lz4_flex 0.12.0 → 0.12.1 (#908)
- Security dependency updates (#916)

---

## [v1.33.14] — 2026-03-05

### Fixes
- Check epoch boundary correctly when updating operator cache and slot timestamp (#890)

---

## [v1.33.13] — 2026-03-04

### Features
- Permissionless: anchor transaction support and proposal manager integration (#886)
- Implement `proposal_id` caching for L2 blocks (#885)
- URC CLI Dockerfile (#881)

---

## [v1.33.11] — 2026-03-04

### Features
- Permissionless: new preconf block API, updated to Rust 1.93 (#884)
- Permissionless: send Shasta proposals to permissionless node (#883)
- Permissionless: integrate `publish_preconf` function (#873)
- Publish preconf commitment to p2p (#868)
- Add `is_forced_inclusion` field to `BuildPreconfBlockResponse` (#880)
- **Config**: new `watchdog_max_counter` configuration parameter (#879)
- Permissionless preconf fixes (#877)

### Fixes
- More descriptive error context for L1 operations (#874)

---

## [v1.33.5] — 2026-02-18

### Features
- Permissionless: expose L2 slot info via API (#869)
- Add `highest_unsafe_payload` alias for taiko-client-rs compatibility (#867)

### Fixes
- Apply `extra_gas_percentage` from configuration correctly (#866)
- Improve `get_l2_height_from_l1` reliability (#865)
- Improve blob reading performance (#863)

---

## [v1.33.0] — 2026-02-12

### Features
- L2 reorg metrics (#862)
- Recover forced inclusions that the node itself produced (#861)
- Permissionless: operator module (#858)
- Insert forced inclusions when not in the Submitter role (#836)

### Fixes
- Fix FI blob decoding (#843)
- Fix calldata missing `0x` prefix (#857)
- Improve data encoding for forced inclusion handling (#855)

### Dependencies
- Use new beacon API endpoint for blob retrieval (#860)

---

## [v1.30.0] — 2026-02-06

### Features
- Fetch operator statuses once per L1 slot instead of per operation (#842)
- **Config**: new `cl_request_timeout` parameter for Consensus Layer RPC requests (#838)
- Fix config output display (#840)

### Fixes
- Remove unnecessary call timeout on driver RPC (#894, backported)

### Dependencies
- Bumped git2 0.20.2 → 0.20.4 (#837)
- Bumped jsonwebtoken 10.2.0 → 10.3.0 (#832)

---

## [v1.29.5] — 2026-02-04

### Fixes
- Fix forced inclusion sync on startup (#835)

### Dependencies
- Bumped jsonwebtoken 10.2.0 → 10.3.0 (#832)

---

## [v1.29.2] — 2026-02-03

### Features
- Check timestamp offset for proposal validity (#830)
- Propose every epoch even without new transactions (#829)
- Shasta: reanchor support (#826)
- Shasta: bridging support (#834)
- Limit forced inclusion block count per proposal (#833)
- Preconfirm multiple forced inclusions in a single batch (#820)
- **Permissionless fork**: introduced as a new fork type (#819)
- Shasta: dynamic `NodeConfig` — `ShastaConfig` fields can now be overridden at runtime (#815)
- Add p2p bootnode Docker image build (#794)

### Fixes
- Recover multiple forced inclusions correctly (#822)
- Fix `taikoAuth_txPoolContentWithMinTip` camelCase response handling (#798)
- Check `proposal_id` correctly during warmup (#799)
- Restart Shasta node on estimation error (#791)
- Log improvements and noise reduction (#816, #823)

### Config Changes
- **`ShastaConfig`**: extended with dynamic node config fields for runtime overrides (#815)
- Refactored config reading with clearer error messages; removed default value for required contract addresses (#792)

### Dependencies
- EIP-7594 blob transaction support with Alloy 1.5 (#809)
- Updated taiko-mono and alethia-reth (#789, #796)
- Updated rustls to 0.23.36 (#810)

---

## [v1.26.0] — 2026-01-06

### Features
- Support Alethia 3.0.0 protocol changes (#782)
- Shasta: updated protocol and new contract addresses (#800, #803)
- Shasta: handle transaction errors with retry logic (#797)
- Refactored preconf block response structure (#785)

### Dependencies
- Updated taiko-mono dependency (#796)
