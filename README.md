<p align="center">
  <img src="assets/novus.png" width="160" alt="Novus">
</p>

<h1 align="center">Novus</h1>

# Novus

**Novus** is an experimental Layer 1 blockchain inspired by the world, factions, and economy of the **RF Online** game.

The project is built in Rust and focuses on an account-based monetary system, lightweight consensus design, and Schnorr signatures.

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
          +----> add sender-race fee to the matching currency in Tax Vault
```

Conceptually:

```text
Amount        = currency quantity in atomic units
Account       = address, per-currency balances, next nonce
Transaction fee = sender race's native currency, credited to Tax Vault
```

---

## Design Goals

Novus aims to remain:

- **Simple** — minimal protocol complexity.
- **Account-based** — balances and transaction nonces are stored per address.
- **Open** — anyone can run a node and participate.
- **Deterministic** — consensus-critical state transitions are reproducible.
- **Cryptographically agile** — the protocol can evolve its cryptographic primitives.
- **Post-quantum oriented** — the project explores post-quantum digital signatures.
- **Experimental** — Novus is a research and development blockchain.

---

## Currency Precision

Every currency uses three decimal places:

```text
1 whole currency = 1,000 atomic units
```

or:

```rust
pub const DECIMALS: u8 = 3;
```

Amounts are represented using an unsigned atomic-unit count inside the shared `Amount` type:

```rust
pub struct Amount(AtomicAmount);
```

This prevents account balances from containing negative amounts.

---

## Account Model

Each address has a balance and next nonce. A transaction signs the sender, nonce,
recipient transfers, fee, and optional message. Nodes require the nonce to match
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
fee
```

The authorization layer contains:

```text
public key
signature
```

This allows Novus to sign a canonical transaction commitment without including the signature itself inside the signed message.

Transaction commitments are domain-separated and bound to the chain context to reduce the risk of cross-network replay.

---

## Consensus

Novus uses a Proof-of-Work design.

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

Difficulty is adjusted over time to keep block production near the target interval.

Novus intentionally keeps its Proof-of-Work design relatively simple rather than introducing block-size-based difficulty adjustments.

---

## Block Reward

Novus uses a fixed block subsidy:

```text
100 BELLY per block
```

The current design does not use Bitcoin-style halvings.

Block emissions are credited directly to the miner's account and are spendable immediately.

Blocks become final after 10,080 confirmations. The node keeps a permanent finalized height and rejects any rollback or peer branch that would replace a block at or below that height. Chain selection among eligible branches still uses cumulative work.

This means miners continue receiving the protocol-defined block subsidy as the chain grows.

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
├── depend/
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

### `depend`

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

Build individual components:

```bash
cargo build -p novus
cargo build -p crypto
cargo build -p wallet
cargo build -p node
```

---

## Check the Workspace

Before running the software, check that all crates compile:

```bash
cargo check
```

For a specific crate:

```bash
cargo check -p novus
cargo check -p wallet
cargo check -p node
```

---

## Run the Node

After building the release version:

```bash
cargo build --release -p node
```

Run the node binary from:

```bash
./target/release/novus
```

During development you can also use:

```bash
cargo run -p novus
```

Node configuration and available command-line options may evolve while Novus is under development.

Check available options with:

```bash
cargo run -p novus -- --help
```

or:

```bash
./target/release/novus --help
```

---

## Run the Wallet

Build the wallet:

```bash
cargo build --release -p wallet
```

Run:

```bash
./target/release/wallet
```

During development:

```bash
cargo run -p wallet
```

The wallet communicates with a running Novus node for blockchain state, balances, transaction broadcasting, and related operations.

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
