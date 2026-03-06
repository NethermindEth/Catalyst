# Fetching a RealTime Proof from Raiko

This document describes the complete mechanism for an external client to request and retrieve a **RealTime fork** proof from a Raiko prover server.

---

## Table of Contents

1. [Overview](#overview)
2. [Server Base URL & API Version](#server-base-url--api-version)
3. [Authentication](#authentication)
4. [Endpoint](#endpoint)
5. [Request Schema](#request-schema)
6. [Response Schema](#response-schema)
7. [Proof Lifecycle & Polling](#proof-lifecycle--polling)
8. [Task Reporting](#task-reporting)
9. [Error Handling](#error-handling)
10. [End-to-End Example](#end-to-end-example)

---

## Overview

The RealTime fork implements **atomic propose+prove**: one proposal per proof per transaction, with no aggregation. The client sends a single POST request containing the L2 block numbers, anchor info, signal slots, derivation sources, and prover configuration. The server performs a two-stage pipeline internally:

1. **Guest input generation** — constructs the provable witness from L1/L2 state.
2. **Proof generation** — runs the selected prover backend (SGX, SP1, RISC0, etc.) on the witness.

The response is **synchronous within a long-poll model**: the server returns immediately with either the completed proof or a status indicating work is in progress. If the proof is not ready, the client re-submits the same request to poll.

---

## Server Base URL & API Version

The RealTime endpoint lives under the **V3 API**. The V3 routes are also merged at the root, so both paths work:

```
POST {BASE_URL}/v3/proof/batch/realtime
POST {BASE_URL}/proof/batch/realtime
```

Where `{BASE_URL}` is the Raiko server address (e.g. `http://localhost:8080`).

---

## Authentication

Authentication is **optional** and depends on server configuration.

### API Key (primary mechanism)

If the server is started with `RAIKO_KEYS` set, all requests require an `X-API-KEY` header.

```
X-API-KEY: raiko_<hex-encoded-32-bytes>
```

Keys are configured server-side via the `RAIKO_KEYS` environment variable as a JSON map of `{ "name": "key_value" }` pairs:

```bash
RAIKO_KEYS='{"my-client":"raiko_abc123..."}'
```

**Rate limiting**: Default 600 requests/minute per key, configurable via `RAIKO_RATE_LIMIT`.

**HTTP error codes for auth failures**:
| Code | Meaning |
|------|---------|
| 401  | Missing, invalid, or inactive API key |
| 429  | Rate limit exceeded |

### JWT Bearer Token (fallback)

If no API key store is configured but a JWT secret is set, the server falls back to `Authorization: Bearer <token>` validation.

### No Auth

If neither is configured, requests are accepted anonymously.

---

## Endpoint

```
POST /v3/proof/batch/realtime
Content-Type: application/json
X-API-KEY: <your-api-key>   (if auth enabled)
```

---

## Request Schema

```jsonc
{
  // === Required fields ===

  // L2 block numbers covered by this proposal.
  // Must be non-empty.
  "l2_block_numbers": [100, 101, 102],

  // Proof backend. One of: "native", "sp1", "risc0", "sgx", "sgxgeth", "tdx", "azure_tdx"
  // Special values: "zk_any" (server picks ZK prover), "sgx_any" (server picks SGX variant)
  "proof_type": "sgx",

  // Highest L1 block number that the L2 derivation references.
  "max_anchor_block_number": 19500000,

  // Hash of the parent proposal, obtained from the on-chain getLastProposalHash().
  // Use 0x0...0 (32-byte zero) for the first proposal after genesis.
  "parent_proposal_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",

  // Percentage of basefee paid to the coinbase address (0-100).
  "basefee_sharing_pctg": 0,

  // === Optional fields (server defaults may apply) ===

  // L2 network identifier. Must match a chain spec configured on the server.
  "network": "taiko_mainnet",

  // L1 network identifier.
  "l1_network": "ethereum",

  // EVM address of the prover (checksummed or lowercase hex).
  "prover": "0xYourProverAddress",

  // Blob proof type. Defaults to "proof_of_equivalence".
  "blob_proof_type": "proof_of_equivalence",

  // L1 signal slots to relay. Array of 32-byte hashes. Defaults to [].
  // When non-empty, the on-chain signalSlotsHash = keccak256(abi.encode(signal_slots)).
  // When empty, signalSlotsHash = bytes32(0).
  "signal_slots": [],

  // Derivation sources for blob data. Defaults to [].
  // Each source maps to the on-chain DerivationSource struct.
  "sources": [
    {
      "isForcedInclusion": false,
      "blobSlice": {
        "blobHashes": ["0x<32-byte-hash>"],
        "offset": 0,
        "timestamp": 1700000000
      }
    }
  ],

  // Previous finalized checkpoint. Null/omitted if none exists yet.
  "checkpoint": {
    "block_number": 99,
    "block_hash": "0x<32-byte-hash>",
    "state_root": "0x<32-byte-hash>"
  },

  // Prover-specific options. Keyed by prover backend name.
  // Only the key matching your proof_type is used.
  "sgx": null,
  "sp1": null,
  "risc0": null,
  "native": null,
  "sgxgeth": null,
  "tdx": null,
  "azure_tdx": null
}
```

### Field Details

| Field | Type | Required | Description |
|---|---|---|---|
| `l2_block_numbers` | `u64[]` | Yes | L2 block numbers in this proposal. Must be non-empty. |
| `proof_type` | `string` | Yes | Prover backend: `native`, `sp1`, `risc0`, `sgx`, `sgxgeth`, `tdx`, `azure_tdx`, `zk_any`, `sgx_any` |
| `max_anchor_block_number` | `u64` | Yes | Highest L1 block the L2 derivation references |
| `parent_proposal_hash` | `bytes32` | Yes | Hash of the parent proposal from `getLastProposalHash()` |
| `basefee_sharing_pctg` | `u8` | Yes | Basefee sharing percentage (0-100) |
| `network` | `string` | No* | L2 network name (server default used if omitted) |
| `l1_network` | `string` | No* | L1 network name (server default used if omitted) |
| `prover` | `string` | No* | Prover EVM address (server default used if omitted) |
| `blob_proof_type` | `string` | No | Defaults to `"proof_of_equivalence"` |
| `signal_slots` | `bytes32[]` | No | L1 signal slots to relay. Defaults to `[]` |
| `sources` | `DerivationSource[]` | No | Blob derivation sources. Defaults to `[]` |
| `checkpoint` | `object \| null` | No | Previous finalized L2 checkpoint |

\* These fields are required for proof generation but can be omitted if the server has defaults configured via its config file or command-line options.

### `zk_any` and `sgx_any` Proof Types

When you set `proof_type` to `"zk_any"` or `"sgx_any"`, the server **draws** (selects) a concrete prover backend based on internal ballot logic. If no prover is drawn, the response returns status `ZKAnyNotDrawn` — this is not an error, it means the server decided not to prove this request. The client should handle this gracefully.

---

## Response Schema

All responses use the V3 `Status` envelope, serialized with `"status"` as a discriminator tag.

### Success — Proof Ready

```json
{
  "status": "ok",
  "proof_type": "sgx",
  "data": {
    "proof": "<proof-string>"
  }
}
```

The `proof` field contains the proof bytes/string as produced by the prover backend. For SGX this is typically a quote; for ZK provers it's the serialized proof.

### Success — Work In Progress

Returned when the proof is still being generated. **Re-submit the same request to poll.**

```json
{
  "status": "ok",
  "proof_type": "sgx",
  "data": {
    "status": "WorkInProgress"
  }
}
```

### Success — Registered

Returned when the task has been registered but not yet started (e.g., guest input generation is still running).

```json
{
  "status": "ok",
  "proof_type": "sgx",
  "data": {
    "status": "Registered"
  }
}
```

### Success — ZK Any Not Drawn

Returned when `proof_type` was `"zk_any"` or `"sgx_any"` and the server decided not to prove this request.

```json
{
  "status": "ok",
  "proof_type": "native",
  "data": {
    "status": "ZKAnyNotDrawn"
  }
}
```

### Error

```json
{
  "status": "error",
  "error": "task_failed",
  "message": "Human-readable error description"
}
```

### Response Status Summary

| `status` | `data` variant | Meaning | Client Action |
|---|---|---|---|
| `"ok"` | `{ "proof": "..." }` | Proof is ready | Extract and use the proof |
| `"ok"` | `{ "status": "Registered" }` | Task queued, guest input generating | Wait and re-submit same request |
| `"ok"` | `{ "status": "WorkInProgress" }` | Proof being generated | Wait and re-submit same request |
| `"ok"` | `{ "status": "Cancelled" }` | Task was cancelled | Do not retry |
| `"ok"` | `{ "status": "ZKAnyNotDrawn" }` | `zk_any`/`sgx_any` not selected | Handle gracefully; no proof produced |
| `"error"` | N/A | Failure | Inspect `error` and `message` fields |

---

## Proof Lifecycle & Polling

The RealTime endpoint uses a **re-submit polling** model, not a separate status endpoint. The flow:

```
Client                                     Server
  |                                          |
  |-- POST /v3/proof/batch/realtime -------->|
  |                                          |-- Stage 1: generate guest input
  |<-- { "status":"ok", data.status:         |
  |      "Registered" } --------------------|
  |                                          |
  |  (wait 5-10 seconds)                     |
  |                                          |
  |-- POST /v3/proof/batch/realtime -------->|  (same request body)
  |                                          |-- Stage 1 complete, Stage 2: prove
  |<-- { "status":"ok", data.status:         |
  |      "WorkInProgress" } -----------------|
  |                                          |
  |  (wait 5-30 seconds)                     |
  |                                          |
  |-- POST /v3/proof/batch/realtime -------->|  (same request body)
  |                                          |-- Proof ready
  |<-- { "status":"ok",                      |
  |      data.proof: "0x..." } --------------|
```

**Key points:**
- Re-submit the **identical** request body each time. The server deduplicates by request key.
- The server internally manages the two-stage pipeline (guest input → proof).
- There is no separate GET endpoint for RealTime proof status.
- Recommended polling interval: 5-30 seconds depending on proof type (native is fast, ZK provers are slow).

---

## Task Reporting

To check the status of all in-flight tasks (including RealTime), use the report endpoint:

```
GET /v3/proof/report
X-API-KEY: <your-api-key>
```

Response is an array of task reports:

```json
[
  {
    "descriptor": {
      "RealTimeGuestInput": {
        "l2_block_numbers": [100, 101],
        "l1_network": "ethereum",
        "l2_network": "taiko_mainnet",
        "parent_proposal_hash": "0x..."
      }
    },
    "status": "WorkInProgress"
  },
  {
    "descriptor": {
      "RealTimeProof": {
        "l2_block_numbers": [100, 101],
        "l1_network": "ethereum",
        "l2_network": "taiko_mainnet",
        "parent_proposal_hash": "0x...",
        "proof_system": "sgx",
        "prover": "0x..."
      }
    },
    "status": "Success"
  }
]
```

RealTime tasks produce two descriptor types in reports:
- `RealTimeGuestInput` — the witness generation stage
- `RealTimeProof` — the actual proof generation stage

---

## Error Handling

### HTTP-Level Errors

| HTTP Code | Cause |
|-----------|-------|
| 400 | Invalid request: missing required fields, bad field types, empty `l2_block_numbers` |
| 401 | Authentication failure (missing/invalid API key) |
| 429 | Rate limit exceeded |
| 500 | Internal server error (prover crash, I/O failure, etc.) |
| 503 | Server capacity full or system paused |

### Application-Level Errors (in response JSON)

These return HTTP 200 but with `"status": "error"`:

```json
{
  "status": "error",
  "error": "task_failed",
  "message": "Task failed with status: AnyhowError(\"RPC timeout\")"
}
```

Common error messages:
- `"l2_block_numbers is empty"` — validation failure
- `"Missing network"` / `"Missing prover"` — required field not provided and no server default
- `"Invalid proof_type"` — unrecognized proof type string
- `"Feature not supported: <proof_type>"` — server not compiled with that prover backend

---

## End-to-End Example

### 1. Health Check

```bash
curl http://localhost:8080/v3/health
# Expected: 200 OK
```

### 2. Submit RealTime Proof Request

```bash
curl -X POST http://localhost:8080/v3/proof/batch/realtime \
  -H "Content-Type: application/json" \
  -H "X-API-KEY: raiko_your_key_here" \
  -d '{
    "l2_block_numbers": [100, 101, 102],
    "proof_type": "sgx",
    "network": "taiko_mainnet",
    "l1_network": "ethereum",
    "prover": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    "max_anchor_block_number": 19500000,
    "parent_proposal_hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    "basefee_sharing_pctg": 0,
    "signal_slots": [],
    "sources": [],
    "checkpoint": null,
    "blob_proof_type": "proof_of_equivalence",
    "sgx": null
  }'
```

### 3. Poll Until Proof Ready

```bash
# Response will be one of:
# { "status": "ok", "proof_type": "sgx", "data": { "status": "Registered" } }
# { "status": "ok", "proof_type": "sgx", "data": { "status": "WorkInProgress" } }
# { "status": "ok", "proof_type": "sgx", "data": { "proof": "0x..." } }

# Re-submit the same request body until data.proof is present.
```

### 4. Pseudocode Client Loop

```python
import requests, time

RAIKO_URL = "http://localhost:8080/v3/proof/batch/realtime"
HEADERS = {
    "Content-Type": "application/json",
    "X-API-KEY": "raiko_your_key_here",
}

payload = {
    "l2_block_numbers": [100, 101, 102],
    "proof_type": "sgx",
    "network": "taiko_mainnet",
    "l1_network": "ethereum",
    "prover": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    "max_anchor_block_number": 19500000,
    "parent_proposal_hash": "0x" + "00" * 32,
    "basefee_sharing_pctg": 0,
    "signal_slots": [],
    "sources": [],
    "checkpoint": None,
    "blob_proof_type": "proof_of_equivalence",
}

while True:
    resp = requests.post(RAIKO_URL, json=payload, headers=HEADERS)
    body = resp.json()

    if body["status"] == "error":
        raise Exception(f"Proof failed: {body['message']}")

    data = body["data"]

    # Proof returned
    if "proof" in data:
        proof = data["proof"]
        print(f"Proof received: {proof[:40]}...")
        break

    # zk_any/sgx_any not drawn — no proof will be produced
    if data.get("status") == "ZKAnyNotDrawn":
        print("Prover not drawn for this request")
        break

    # Still in progress
    status = data.get("status", "Unknown")
    print(f"Status: {status}, polling again...")
    time.sleep(10)
```

---

## Appendix: DerivationSource Schema

The `sources` array contains objects matching the on-chain `DerivationSource` struct:

```jsonc
{
  // Whether this source is from a forced inclusion.
  // Always false for RealTimeInbox proposals.
  "isForcedInclusion": false,

  // Blob slice referencing the transaction data.
  "blobSlice": {
    // Array of 32-byte blob hashes (versioned hashes from the blob transaction).
    "blobHashes": ["0x01<...>"],
    // Byte offset within the blob where this source's data begins.
    "offset": 0,
    // Timestamp associated with the blob.
    "timestamp": 1700000000
  }
}
```

## Appendix: Checkpoint Schema

```jsonc
{
  // L2 block number of the checkpoint.
  "block_number": 99,
  // Block hash at that L2 block.
  "block_hash": "0x<32-byte-hex>",
  // State root at that L2 block.
  "state_root": "0x<32-byte-hex>"
}
```

## Appendix: Supported Proof Types

| Value | Backend | Description |
|---|---|---|
| `"native"` | Native | Block construction + equality check (no cryptographic proof) |
| `"sp1"` | SP1 | Succinct SP1 zero-knowledge prover |
| `"risc0"` | RISC0 | RISC Zero zero-knowledge prover |
| `"sgx"` | Intel SGX | Trusted execution environment proof |
| `"sgxgeth"` | SGX + Geth | SGX with Geth execution client |
| `"tdx"` | Intel TDX | Trust Domain Extensions proof |
| `"azure_tdx"` | Azure TDX | Azure Confidential VM (TDX) proof |
| `"zk_any"` | Server-selected ZK | Server picks between SP1/RISC0 via ballot |
| `"sgx_any"` | Server-selected SGX | Server picks between SGX/SGXGeth via ballot |
