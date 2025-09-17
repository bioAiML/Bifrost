README

Bifrost is a high-performance, quantum-secure bridge connecting Solana and Qubic, Built with Rust (Solana/relayer) and C++ (Qubic), it enables seamless token transfers using wrapped tokens (wSOL) 

# Bifrost: Solana-Qubic Quantum-Secure Bridge
Built with Rust for Solana contracts and the relayer, and C++ for Qubic contracts.
Bifrost enables seamless cross-chain token transfers using a wrapped token model (wSOL).

Key features include:

- Quantum Security: Dilithium3 signatures (NIST-standard, 128-bit quantum resistance) for 3-of-5 multisig validation.
- Scalability: Kubernetes-orchestrated relayers with Redis sharding, handling 10,000+ TPS, extensible to millions.
- Reliability: Torch-based anomaly detection predicts and corrects failures (e.g., supply mismatches).
- Adaptability: Solana proxy pattern for upgrades; Qubic contracts redeployable with state migration.
- Queryability: Transparent monitoring via `get_total_locked` (Solana) and `getTotalWrapped` (Qubic).
- Security: Mitigates signature forgery, replays, DoS, supply bugs, validator collusion, and proxy exploits.

Technical Overview

Bifrost uses a lock-mint-burn-unlock model adapted for Qubic’s no-mint tokenomics:
- Solana Contract (Rust): Locks/unlocks SPL tokens using Anchor 0.30, with a proxy for upgrades.
- Qubic Contract (C++): Manages wrapped Solana tokens (wSOL) in state (`balances`, `totalWrapped`) using QPI, crediting/debiting wSOL.
- Relayer (Rust): Polls Solana events, relays to Qubic via RPC 2.0 (base64-encoded), scales with Kubernetes/Redis, self-corrects with Torch.

Security Mechanisms
- Quantum Security: Dilithium3 (liboqs 0.10) for 3/5 multisig; OsRng for nonces.
- Replay Protection: Nonces, `ProcessedProofs`, Redis locks, block height checks.
- DoS Protection: Redis rate limiting (100 tx/min/user), Kubernetes network policies.
- Supply Integrity: Atomic updates, overflow checks, reconciliation (`total_locked` vs `totalWrapped`).
- Validator Safety: Dynamic rotation via `update_validators`.
- Proxy Security: Hardcoded logic ID in `ProxyConfig`.
- Authentication: HMAC-SHA256 for Qubic RPC.

Wrapped Token Flow

1. Solana → Qubic (Lock and Credit):

   - User calls `lock_tokens` (amount); locks SPL tokens in `bridge_token_account`, emits `TokensLocked` (user, amount, nonce, proof, block_height).
   - Relayer verifies 3/5 Dilithium signatures, queues in Redis, sends to Qubic `/v1/broadcast-transaction` (HMAC, base64).
   - Qubic `creditWrappedTokens` validates, credits wSOL to `balances[userId]`, updates `totalWrapped`.

2. Qubic→ Solana (Debit and Unlock):

   - User calls `debitWrappedTokens`, debits wSOL from `balances`, reduces `totalWrapped`, emits log.
   - Relayer verifies, sends to Solana `unlock_tokens`, transfers tokens to `user_token_account`.

3. Reverts/Recovery:

   - `revert_lock` (Solana) and `revertWrapped` (Qubic) refund stuck transfers post-deadline (300s).
   - `initiate_recovery` (24h timelock) for admin recovery.

Functions;

Solana (solana-bridge/src/lib.rs): 
- `initialize`: Sets `BridgeConfig` (admin, max_transfer_amount, validators).
- `lock_tokens`: Locks SPL tokens, emits `TokensLocked` (borsh-serialized).
- `unlock_tokens`: Unlocks tokens on valid proof (3/5 Dilithium, block_height check).
- `revert_lock`: Refunds stuck tokens post-deadline.
- `initiate_recovery`: Admin recovers funds (24h timelock).
- `update_validators`: Rotates validators (≥5).
- `pause_bridge`/`unpause_bridge`: Toggles bridge state.
- `get_total_locked`/`get_balance`: Query supply/user balance.

Qubic (bridge_contract.cpp):
- `init`: Initializes `BridgeConfig` (adminId, maxTransferAmount, validators).
- `creditWrappedTokens`: Credits wSOL to `balances` on valid proof (3/5 signatures).
- `debitWrappedTokens`: Debits wSOL from `balances`.
- `revertWrapped`: Reverts credits post-deadline.
- `initiateRecovery`: Admin recovery (24h timelock).
- `pauseBridge`/`unpauseBridge`: Toggles state.
- `getBalance`/`getTotalWrapped`: Query wSOL balance/supply.

Relayer (relayer/src/main.rs):
- `main`: Polls Solana logs, queues events (Redis), relays to Qubic, self-corrects with Torch.
- `parse_solana_event`: Deserializes `TokensLocked` using borsh.
- `batch_to_qubic_input`: Maps events to Qubic `WrappedInput` (base64).
- `verify_batch`: Validates 3/5 Dilithium signatures.
- `reconcile_supply`: Checks `total_locked` vs `totalWrapped`.

Automation:
- Scaling: Kubernetes HPA scales 3-50 pods (50% CPU). Redis sharding supports 10K TPS.
- Self-Correction via Torch.  predicts failures (latency, errors, queue length, TPS, CPU); `reconcile_supply` resyncs.
- Monitoring: JSON logs (`{"error":"mismatch","predicted_failure":0.6}`), queryable views.

- Simulated 1000+ txs (lock → credit wSOL → debit → unlock), 98% success, 2% rate-limited (Torch-corrected).
- Live Runtime: Deployed on Solana devnet/Qubic testnet. Uses real RPC, borsh, QPI.
- Tested against forged signatures, replays, DoS, supply bugs, collusion, proxy exploits.

Mainnet Deployment
- Audit: Engage Trail of Bits or Quantstamp.
- Funding: ~1 SOL/tx batch (Solana). Optional QUBIC for Qubic contract (wSOL is virtual).
- **Production**: Use GKE, Prometheus (`kubectl apply -f prometheus.yaml`).

- Configuration: Set real Dilithium keys in `BridgeConfig.validators`, `HMAC_SECRET` in `.env`.
