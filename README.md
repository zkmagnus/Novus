# Novus

**Novus** is an experimental Layer 1 blockchain inspired by the world, factions, and economy of the **RF Online** game.

The project is built in Rust and focuses on an account-based monetary system, Proof of Work, and BIP340 Schnorr signatures.

> Novus is an independent project and is not affiliated with, endorsed by, or associated with the owners of the RF Online intellectual property or any official RF Online license holder. RF Online is referenced only to describe the project's inspiration.

---

## Overview

The RF fork uses an account-based native-coin ledger.

Each account stores a balance and the next transaction nonce accepted from its address.

```text
Account balance + nonce
          |
          v
 Signed account transaction
          |
          +----> debit sender and advance nonce
          +----> credit recipients
          +----> protocol tax is collected where the transaction type is taxable
          +----> Novus transfer fee goes directly to the including block miner
```

Conceptually:

```text
Amount        = currency quantity in atomic units
Account       = address, per-currency balances, next nonce
Transaction fee = only Novus transfers, paid in Novus to the including block miner
Transaction tax = rates and denomination follow the transfer or conversion policy below
```

---

## Design Goals

Novus aims to remain:

- **Simple** — minimal protocol complexity.
- **Account-based** — balances and transaction nonces are stored per address.
- **Open** — anyone can run a node and participate.
- **Deterministic** — consensus-critical state transitions are reproducible.
- **Cryptographically modular** — hashing and signature code live in the `crypto` crate.
- **Experimental** — Novus is a research and development blockchain.

---

## Currency Precision

CP, Dalant, Disena, Gold, and Novus each use six decimal places:

```text
1 whole currency = 1,000,000 atomic units
```

or:

```rust
pub const DECIMALS: u8 = 6;
```

Amounts are represented using an unsigned atomic-unit count inside the shared `Amount` type:

```rust
pub struct Amount(AtomicAmount);
```

This prevents account balances from containing negative amounts.

---

## Account Model

Each address has a balance and next nonce. A transaction signs the sender, nonce,
recipient transfers, a Novus-only fee, and optional message. Nodes require the nonce to match
the account state, debit the sender atomically, and credit each recipient. Block
rewards credit the miner account directly. Rollbacks restore account snapshots.

---

## Transaction Authorization

Transaction data and authorization are separated.

The transaction intent contains the economic operation:

```text
sender
nonce
transfers
protocol-calculated tax
```

The signed tax amount is denominated in the transaction's source currency. Wallets show it before submission, and nodes recompute it from the active protocol rules and reject a transaction if the signed amount is stale or incorrect.

The authorization layer contains:

```text
public key
signature
```

This allows Novus to sign a canonical transaction commitment without including the signature itself inside the signed message.

Transaction commitments are domain-separated and bound to the chain context to reduce the risk of cross-network replay.

---

## Consensus

Novus uses Bitcoin-style double SHA-256 (SHA-256d) Proof of Work over block headers.

The current design targets an average block interval of approximately:

```text
30 seconds
```

Mining follows a conventional:

```text
block data
   +
nonce
   |
   v
 hash
   |
   v
difficulty target
```

model.

Chip War epochs last 3 days (8,640 blocks at the 30-second target). Tax and exchange outcomes settle at each epoch boundary. Proof-of-work difficulty uses ASERT with a one-hour half-life, anchored at block 1, and recalculates for each block. Network difficulty has a hard ceiling of 16x its starting value; if hash power grows beyond that ceiling, blocks may arrive faster than the 30-second target. ASERT targets the long-run average and does not guarantee that each block takes 30 seconds. Block timestamps must be after the median time of recent blocks and no more than two hours ahead of local time.

---

## Block Reward

The block subsidy is deterministically derived from the previous block hash and ranges inclusively from:

```text
100 to 1,000 coins per block
```

The current design has no per-block emission target or Bitcoin-style halving schedule. Because miners can try different candidate blocks and parent hashes, this reward method is verifiable but can be influenced through hash grinding.

Block emissions are credited directly to the miner's account and are spendable immediately.

Blocks become final after 10,080 confirmations. The node keeps a permanent finalized height and rejects any rollback or peer branch that would replace a block at or below that height. Chain selection among eligible branches still uses cumulative work.

This means miners continue receiving the protocol-defined block subsidy as the chain grows.

Novus is a global currency. Gold converts one way into Novus at a fixed rate of 1,000 Gold to 1 Novus; Novus cannot be converted back to Gold. Gold transfers are restricted to accounts of the sender's race, while Novus transfers can cross races. The base exchange rate for each race's native currency is 10,000 units per Gold, adjusted by Chip War by up to 20%. Chip War tax adjustments apply to every taxable transaction type, using the sender's race. Base tax is 1% for race-currency transfers and Gold transfers, 3% for race currency to Gold conversion, 1% for Gold to race currency conversion, and 5% for Gold to Novus conversion. The shared Chip War tax modifier starts at 0 percentage points. A loss or failure adds 1 point, cumulatively across both outcomes, up to +3 points; a win subtracts 1 point down to a zero modifier. This makes the maximum rates 4%, 6%, 4%, and 8%, respectively. Taxes are denominated in the source currency and held in the originating race's vault, including Gold taxes; only that race's miners can claim them. Novus transfers pay a size-based fee in Novus directly to the including block miner and are exempt from protocol tax. Other transactions have no relay fee.

The initial global exchange vault holds 100 million Gold. Gold converted to Novus is burned. After each cumulative 100 million Gold converted, the protocol issues 100 million Gold back into the global exchange vault. Conversion progress, total Gold burned, and total Novus minted are consensus state and are available from `/status`.

---

## Cryptography

Novus is designed with cryptographic agility in mind.

Transaction authorization uses BIP340 Schnorr signatures on secp256k1.

Cryptographic components are isolated so that signature and hashing implementations can evolve without requiring unrelated parts of the protocol to be redesigned.

> Novus is experimental software. Its cryptographic implementation should not be considered production-audited.

---

## Project Structure

The workspace is organized into several Rust crates:

```text
novus/
├── common/
├── crypto/
├── novus/
├── runtime/
├── wallet/
├── dependency/
└── Cargo.toml
```

### `common`

Shared protocol types such as:

```text
ChainContext
Height
Nonce
```

### `crypto`

Cryptographic primitives including:

```text
addresses
hashing
public keys
signatures
verification
```

### `novus`

Consensus-critical blockchain logic:

```text
blockchain
consensus
genesis
ledger
transactions
Account state
emission
```

### `runtime`

Node runtime and networking components.

### `wallet`

Wallet functionality for creating keys, checking balances, constructing transactions, and interacting with a Novus node.

### `dependency`

Vendored dependencies used by the workspace.

---

## Requirements

Novus currently targets Rust edition 2024.

Recommended environment:

```text
Rust 1.90+
Cargo
Git
Linux
```

Check your Rust installation:

```bash
rustc --version
cargo --version
```

Install Rust using the official Rust toolchain if it is not already installed.

---

## Clone

```bash
git clone https://github.com/zkmagnus/Novus.git
cd novus
```

---

## Build

Build the complete workspace:

```bash
cargo build
```

For an optimized build:

```bash
cargo build --release
```

Build the three race node binaries in release mode:

```bash
cargo build --release -p accretia
cargo build --release -p bellato
cargo build --release -p cora
```

---

## Check the Workspace

Check that the complete workspace compiles:

```bash
cargo check --workspace
```

---

## Run the Node

Each race node is a separate binary. For example, run the Accretia node with RPC on the default local port `10593`:

```bash
cargo run --release -p accretia -- run
```

The RPC listener defaults to `127.0.0.1:10593`; the P2P listener defaults to `0.0.0.0:10999`. Override them with `--rpc` and `--p2p`; set the database directory with `--data`:

```bash
cargo run --release -p accretia -- run --data ./data/accretia --rpc 127.0.0.1:10593
```

Use `bellato` or `cora` instead of `accretia` to run the other race variants. To run only the RPC server against a database:

```bash
cargo run --release -p accretia -- rpc ./data/accretia 127.0.0.1:10593
```

View the node commands with:

```bash
cargo run -p accretia -- help
```

The wallet and node default RPC address is `127.0.0.1:10593`.

### Read-only RPC views

The RPC exposes `/status` (including aggregate tax-vault balances), `/tax-vault`
and `/tax-vault/{race}` (unclaimed tax by originating race and currency),
`/war` and `/war/{race}` (epoch progress and active participant counts), and
`/stake/{address}` (a wallet's active stake, current-epoch participation,
cumulative contribution total, and persistent race rank). Each wallet earns at
most one non-transferable contribution point per epoch, either by having active
stake or by submitting `wallet stake heartbeat`; the heartbeat does not require
finding a block. Full request and response details are served at `/docs` and
`/openapi.json`.

---

## Wallet Commands

Wallet operations are subcommands of each race node binary; there is no separate wallet executable or interactive menu. The wallet library remains a workspace crate used by the node. Create a wallet, then check its address and balance:

```bash
cargo run -p accretia -- wallet new --wallet ./wallet.json
cargo run -p accretia -- wallet address --wallet ./wallet.json
cargo run -p accretia -- wallet balance --wallet ./wallet.json --rpc 127.0.0.1:10593
```

Send 1.25 units of the wallet's race currency (or use `--currency novus` for Novus):

```bash
cargo run -p accretia -- wallet send --to <recipient-address> --amount 1.25 --currency cp --wallet ./wallet.json --rpc 127.0.0.1:10593
```

Send Novus or convert Gold to Novus:

```bash
cargo run -p accretia -- wallet send --to <recipient-address> --amount 2 --currency novus --wallet ./wallet.json
cargo run -p accretia -- wallet convert --amount 1 --currency gold --to novus --wallet ./wallet.json
```

Convert the wallet's race currency to Gold with `--to gold` (use `cp`, `dlt`, or `dsc` to match the wallet's race):

```bash
cargo run -p accretia -- wallet convert --amount 1000 --currency cp --to gold --wallet ./wallet.json
```

Gold converts one way to Novus at **1,000 Gold = 1 Novus**.

Use `--offline` to print a signed transaction as hex without submitting it. CLI amounts use coin units with up to six decimal places; JSON RPC balances use atomic units. For example, `1.25` coins equals `1,250,000` atomic units. The node also provides `node balance` as an alias for `node account` when using its race binary, and serves the interactive RPC reference at `http://127.0.0.1:10593/docs` and the raw OpenAPI document at `http://127.0.0.1:10593/openapi.json`.

---

## Development

Run formatting:

```bash
cargo fmt
```

Run checks:

```bash
cargo check
```

Run tests:

```bash
cargo test
```

For all workspace crates:

```bash
cargo test --workspace
```

Useful before committing:

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

---

## Experimental Status

Novus is currently under active development.

Expect:

- consensus changes,
- serialization changes,
- database resets,
- wallet format changes,
- network incompatibilities between versions,
- cryptographic changes,
- breaking API changes.

Do not treat the current network or wallet format as permanently stable.

Do not use Novus to store funds you cannot afford to lose.

---

## Inspiration

The name and theme of Novus are inspired by **One Piece** and its world of adventure.

The project takes inspiration from themes such as:

```text
Freedom
Adventure
Exploration
Friendship
Decentralization
```

The technical protocol itself is independently designed and implemented as an experimental blockchain project.

---

## Repository

Novus is developed openly on GitHub:

```text
https://github.com/zkmagnus/Novus
```

Contributions, testing, bug reports, protocol discussion, and code review are welcome.

---

## License

Novus is released under the **MIT License**.

See the repository's `LICENSE` file for details.

---

## Disclaimer

Novus is experimental blockchain software intended for research, development, and education.

The software is provided without warranty. Consensus rules, cryptography, networking, wallet behavior, and economic parameters may change during development.

**Novus is not affiliated with the official One Piece franchise or its rights holders.**
