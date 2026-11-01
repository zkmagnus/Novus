# Vendored Schnorr dependencies

Sources copied from crates.io for offline builds:

- `secp256k1` 0.29.1, CC0-1.0, from rust-bitcoin/rust-secp256k1
- `secp256k1-sys` 0.10.1, CC0-1.0, from rust-bitcoin/rust-secp256k1 (includes upstream libsecp256k1 C source)
- `cc` 1.10.0, MIT OR Apache-2.0
- `find-msvc-tools` 0.1.15, MIT OR Apache-2.0
- `shlex` 2.0.1, MIT OR Apache-2.0

Cargo manifests for these packages point to the neighboring vendored dependencies. The two secp256k1 manifests also declare legacy cfg names for the workspace's deny-level `unexpected_cfgs` lint.
