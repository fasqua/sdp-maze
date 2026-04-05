# SDP Maze (Stealth Diffusion Protocol - Maze)

A privacy-focused transaction routing protocol built on Solana. SDP Maze breaks on-chain links between source and destination through dynamic maze topology.

## Table of Contents

- [Overview](#overview)
- [Why SDP Maze](#why-sdp-maze)
- [Features](#features)
- [How It Works](#how-it-works)
- [Privacy Model](#privacy-model)
- [Security](#security)
- [Technical Architecture](#technical-architecture)
- [API Reference](#api-reference)
- [Getting Started](#getting-started)
- [Links](#links)
- [License](#license)

## Overview

SDP Maze provides privacy for Solana transactions through a novel approach: instead of linear hop chains, it creates a maze of splits and merges that obscures the transaction path.

The protocol is designed with three core principles:

1. **Privacy by Design** - No direct on-chain link between source and destination
2. **Fund Safety** - Encrypted keypairs and comprehensive recovery system
3. **User Sovereignty** - Only the wallet owner can recover stuck funds

## Why SDP Maze

Traditional privacy solutions on blockchain have limitations:

| Approach | Limitation |
|----------|------------|
| Simple Mixers | Predictable patterns, timing correlation |
| Fixed Pool Sizes | Amount correlation attacks |
| Centralized Tumblers | Trust requirements, single point of failure |
| Linear Hop Chains | Easy to trace with sufficient resources |

SDP Maze addresses these by:

- **Dynamic Topology** - Every transaction creates a unique maze structure
- **Variable Splits** - Amounts are divided unpredictably across paths
- **Random Merges** - Funds recombine at random points
- **Fresh Keypairs** - Each node uses a one-time address
- **Non-Custodial** - Relay never holds unencrypted keys

## Features

### Private Transfer

Send SOL to any SDP-compatible address (`kl_` prefix). Your transaction passes through a unique maze of intermediate wallets.

Key characteristics:
- Minimum 7 hops, dynamically expanded based on amount
- Random split points divide funds across parallel paths
- Merge points recombine funds before final delivery
- One-time keypairs for each intermediate node

### Private Swap

Exchange SOL for any SPL token (USDC, BONK, etc.) with full privacy. The swap executes at the final maze node via Jupiter.

Supported tokens:
- Any SPL token available on Jupiter
- Token-2022 standard supported
- Automatic ATA (Associated Token Account) creation

### Split / Diversify

Distribute SOL across multiple destination wallets through independent maze routes.

Distribution modes:
- **Equal** - Split evenly across all destinations
- **Weighted** - Custom percentage per destination

### Recovery System

Blockchain transactions can fail for various reasons (network congestion, RPC issues, etc.).

- **Automatic Detection** - Failed transactions are marked for recovery
- **Full Fund Recovery** - All SOL stuck in any maze node can be recovered
- **Ownership Validation** - Only the original sender can recover funds
- **Single Command** - Simple `recover [request_id]` interface

**Important:** You must register at least one destination wallet (`add1 [address]`) before using SDP Maze. Recovery funds are sent to your first registered wallet (slot 1) by default.

## How It Works

### Maze Generation

When you initiate a transfer, SDP Maze generates a unique directed acyclic graph (DAG):

```
        [Deposit Node]
              |
        +-----+-----+
        |           |
    [Node 1]    [Node 2]
        |           |
    +---+---+   +---+---+
    |   |   |   |   |   |
  [N3] [N4] [N5] [N6]
    |   |   |   |
    +---+---+   +---+---+
        |           |
    [Node 7]    [Node 8]
        |           |
        +-----+-----+
              |
        [Merge Node]
              |
        [Destination]
```

The maze structure is determined by:

1. **Amount** - Larger amounts generate more complex mazes
2. **Randomness** - Cryptographic randomness determines topology
3. **Minimum Security** - At least 7 nodes regardless of amount

### Transaction Flow

1. **Request Creation**
   - User specifies destination and amount
   - Relay generates unique maze topology
   - Fresh keypairs created for each node
   - Keypairs encrypted with AES-256-GCM

2. **Deposit**
   - User sends SOL to one-time deposit address
   - Relay monitors for incoming transaction

3. **Maze Execution**
   - Funds flow level by level through the maze
   - Each node transfers to its designated outputs
   - Splits divide funds, merges recombine them

4. **Completion**
   - Final node transfers to destination
   - For swaps: Jupiter executes the token exchange
   - Transaction signatures recorded for verification

### Fee Structure

- **Protocol Fee**: 0.5% of transfer amount
- **Network Fees**: ~0.000005 SOL per node transaction
- **Pro Subscriber**: 0% protocol fee (requires active subscription)
- **Network Buffer**: 0.01 SOL safety buffer for diversify operations

### Custom Maze Configuration (KAUSA Holders)

Users holding 1,000,000+ KAUSA tokens can customize their maze parameters:

| Parameter | Range | Default | Description |
|-----------|-------|---------|-------------|
| Hop Count | 5-10 | 10 | Number of intermediate nodes |
| Split Ratio | 1.1-3.0 | 1.618 (Golden Ratio) | How funds split at branch points |
| Merge Strategy | early/late/middle/random/fibonacci | random | When parallel paths recombine |
| Delay Pattern | none/linear/exponential/random/fibonacci | none | Timing between transactions |
| Delay (ms) | 0-5000 | 0 | Base delay in milliseconds |
| Delay Scope | node/level | node | Apply delay per node or per level |

Preferences are saved and automatically applied to future transactions.

## Privacy Model

### What SDP Maze Protects Against

**Transaction Graph Analysis**

On-chain observers cannot establish direct links between source and destination. The maze creates multiple possible paths, and without access to the relay's encrypted database, determining the actual route is computationally infeasible.

**Wallet Clustering**

Common heuristics used to cluster wallets (common inputs, change addresses, timing) are defeated by:
- One-time intermediate addresses
- Random split amounts
- Sequential execution with natural timing variance

**Amount Correlation**

Splits and merges obscure the original amount. A 1 SOL transfer might split into 0.3, 0.25, 0.2, 0.15, 0.1 SOL across different paths, making amount-based correlation difficult.

**Direct Observation**

Without relay database access, an observer sees only:
- Deposit to an unknown address
- Multiple small transfers between unknown addresses
- Final transfer from unknown address to destination

### What SDP Maze Does NOT Protect Against

**Timing Analysis with Full Observation**

An adversary observing the entire blockchain in real-time with unlimited resources could potentially correlate timing. Mitigation: natural variance in transaction confirmation times.

**RPC Provider Logging**

Your RPC provider sees your requests. Mitigation: use a private or self-hosted RPC node.

**Relay Compromise**

If the relay server is compromised, an attacker could potentially decrypt keypairs. Mitigation: encrypted storage, regular key rotation, and the relay never sees unencrypted user wallet keys.

**Endpoint Security**

SDP Maze cannot protect against compromised user devices or wallets.

### Privacy Levels

| Scenario | Privacy Level |
|----------|---------------|
| Casual observer checking blockchain | Very High |
| Chain analysis firm with heuristics | High |
| Targeted analysis with resources | Medium |
| Relay database access | Low |

## Security

### Cryptographic Protections

**Keypair Encryption**

All maze node keypairs are encrypted before storage using AES-256-GCM authenticated encryption. The encryption key is:
- Loaded from environment variables at runtime
- Never hardcoded or committed to version control
- Rotatable without affecting existing transactions

**Deterministic Derivation**

User meta-addresses are derived deterministically from wallet signatures, ensuring:
- Only wallet owners can generate their meta-address
- No central registry of user identities
- Consistent identity across sessions

### Access Controls

**Ownership Validation**

Recovery operations cryptographically verify that the requester owns the original transaction:

1. User provides request ID and destination
2. System retrieves sender's meta-hash from request
3. Verifies destination is registered to that meta-hash
4. Only then executes recovery

This prevents attackers from recovering funds even if they know the request ID.

**Wallet Registration**

Users must register destination wallets before use. Registration is tied to wallet signature, preventing unauthorized additions.

### Transport Security

- All API communications over HTTPS/TLS
- CORS configured for authorized origins only
- Request validation and sanitization

### Operational Security

- No IP address logging for transaction requests
- Automatic pruning of completed transaction data
- Encrypted database at rest

## Technical Architecture

### Components

**Relay Server (Rust/Axum)**

Core responsibilities:
- Maze topology generation
- Keypair generation and encryption
- Transaction construction and signing
- Blockchain interaction via RPC
- Status tracking and recovery
- Jupiter swap integration

**Database (SQLite)**

Stores:
- Request metadata and status
- Encrypted node keypairs
- Maze topology (JSON)
- User wallet registrations

**External Integrations**

- Solana RPC for blockchain interaction
- Jupiter API for token swaps

### Data Flow

```
User Request
     |
     v
+-------------------+
|  Input Validation |
+-------------------+
     |
     v
+-------------------+
|  Maze Generation  |
|  - Topology       |
|  - Keypairs       |
|  - Encryption     |
+-------------------+
     |
     v
+-------------------+
|  Database Storage |
+-------------------+
     |
     v
+-------------------+
| Return Deposit Addr|
+-------------------+
     |
     v
[User Deposits SOL]
     |
     v
+-------------------+
|  Deposit Monitor  |
+-------------------+
     |
     v
+-------------------+
|  Maze Execution   |
|  - Level by level |
|  - Split/Merge    |
+-------------------+
     |
     v
+-------------------+
|  Final Transfer   |
|  or Jupiter Swap  |
+-------------------+
     |
     v
+-------------------+
| Status: Completed |
+-------------------+
```

## API Reference

### Create Transfer

**POST /api/v1/transfer**

Request:
```json
{
  "sender_meta_hash": "string",
  "receiver_meta": "string (kl_address)",
  "amount_sol": number,
  "hop_count": number (optional),
  "maze_config": { ... } (optional, for KAUSA holders)
}
```

Response:
```json
{
  "request_id": "string",
  "deposit_address": "string",
  "amount_lamports": number,
  "fee_lamports": number,
  "expires_at": number,
  "maze_preview": {
    "total_nodes": number,
    "total_levels": number
  }
}
```

### Create Swap

**POST /api/v1/swap/request**

Request:
```json
{
  "sender_meta_hash": "string",
  "amount_sol": number,
  "token_mint": "string (SPL token address)",
  "destination": "string (wallet address)",
  "maze_config": { ... } (optional, for KAUSA holders)
}
```

Response:
```json
{
  "success": boolean,
  "request_id": "string",
  "deposit_address": "string",
  "deposit_amount": number,
  "fee": number,
  "expires_in": number,
  "maze_preview": {
    "total_nodes": number,
    "total_levels": number
  }
}
```

### Create Diversify

**POST /api/v1/diversify/request**

Request:
```json
{
  "meta_address": "string",
  "total_amount": number,
  "distribution_mode": "equal" | "percentage" | "fixed",
  "maze_config": { ... } (optional, for KAUSA holders),
  "routes": [
    {
      "destination_slot": number,
      "percentage": number
    }
  ]
}
```

Response:
```json
{
  "success": boolean,
  "request_id": "string (div_ prefix)",
  "deposit_address": "string",
  "total_amount": number,
  "fee_amount": number,
  "routes": [
    {
      "route_index": number,
      "destination": "string",
      "amount": number
    }
  ],
  "expires_in": number
}
```

### Get Status

**GET /api/v1/transfer/:request_id**

Response:
```json
{
  "request_id": "string",
  "status": "pending" | "deposit_received" | "processing" | "completed" | "failed" | "partial",
  "deposit_address": "string",
  "amount_lamports": number,
  "progress": {
    "completed_nodes": number,
    "total_nodes": number,
    "percentage": number
  },
  "final_tx_signature": "string | null",
  "route_signatures": [
    {
      "route_index": number,
      "destination": "string",
      "tx_signature": "string",
      "status": "string"
    }
  ]
}
```

### Recover Funds

**POST /api/v1/recover**

Request:
```json
{
  "request_id": "string",
  "destination": "string (must be registered to sender)"
}
```

Response:
```json
{
  "success": boolean,
  "recovered_amount": number,
  "tx_signatures": ["string"],
  "error": "string | null"
}
```

### Maze Preferences (KAUSA Holders)

**POST /api/v1/preferences/get**
```json
{
  "meta_address": "string"
}
```

Response:
```json
{
  "success": true,
  "preferences": {
    "hop_count": 10,
    "split_ratio": 1.618,
    "merge_strategy": "random",
    "delay_pattern": "none",
    "delay_ms": 0,
    "delay_scope": "node",
    "updated_at": 1234567890
  }
}
```

**POST /api/v1/preferences/save**
```json
{
  "meta_address": "string",
  "hop_count": 10,
  "split_ratio": 1.618,
  "merge_strategy": "random",
  "delay_pattern": "none",
  "delay_ms": 0,
  "delay_scope": "node"
}
```

### Wallet Management

**POST /api/v1/wallet/add**
```json
{
  "owner_meta_hash": "string",
  "slot": number (1-5),
  "wallet_address": "string"
}
```

**POST /api/v1/wallet/delete**
```json
{
  "owner_meta_hash": "string",
  "slot": number
}
```

**POST /api/v1/wallet/list**
```json
{
  "owner_meta_hash": "string"
}
```

## Getting Started

### Using SDP Maze

1. **Connect Wallet**
   - Visit kausalayer.com/maze
   - Connect your Solana wallet (Phantom, Solflare, Backpack, etc.)
   - Sign the authentication message

2. **Register Destination Wallets**
   - Type `add1 [wallet-address]` to register your first destination
   - You can register up to 5 destination wallets (slots 1-5)
   - Use `wallets` to view registered wallets

3. **Available Commands**
   - `info` - View all available commands
   - `send [amount] SOL to [kl_address]` - Private transfer
   - `swap [amount] SOL to [token_mint]` - Private swap
   - `split [amount] SOL` - Diversify to multiple wallets
   - `recover [request_id]` - Recover stuck funds
   - `wallets` - List registered wallets

4. **Execute Transaction**
   - After initiating, you'll receive a deposit address
   - Send the exact amount shown (includes fees and network costs)
   - Wait for maze execution to complete
   - View transaction proof on Solscan

### Example Commands

**Private transfer**
```
send 1 SOL to kl_abc123...
```

**Swap to USDC**
```
swap 0.5 SOL to EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v
```

**Split to 3 wallets equally**
```
split 3 SOL
> Select wallets: 1,2,3
> Mode: equal
```

**Recover stuck funds**
```
recover swap_abc123def456
```

## Links

- **Website**: https://kausalayer.com
- **SDP Maze App**: https://kausalayer.com/maze
- **Twitter/X**: https://x.com/kausalayer
- **KAUSA Token**: https://solscan.io/token/BWXSNRBKMviG68MqavyssnzDq4qSArcN7eNYjqEfpump

## License

Apache License 2.0

---

*Privacy for everyone.*
