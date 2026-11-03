# Stake-supported PoW proposal (prototype parameters)

This document records the first implementation assumptions for review. It is
not compatible with existing Novus databases or peers: activation requires a
new chain specification and coordinated network upgrade.

## Scope

- Race-native coin delegated to a race pool supports every miner of that race
  as a voluntary contribution, without a stake reward.
- Mining stake has no finality vote, no slash condition, and no effect on the
  existing finality rule.
- The block producer receives all transaction fees and all tax distributions
  currently paid to miners.
- Stake is not intended to resist Sybil identities.

## Initial economic assumptions

These values are deliberately provisional and must be reviewed before any
mainnet activation:

- `S_ref`: 100,000 whole units of the miner's race currency.
- `CAP`: 100% boost, so the maximum target multiplier is 2x.
- Active stake is the total delegated amount in the race pool, as recorded in
  the candidate block's parent state. Delegations activate after one complete
  Chip War epoch.
- `boost = min(active_stake / S_ref, 1)` using integer fixed-point arithmetic.
- Every miner in the race uses `effective_target = min(network_target * (1 + boost), pow_limit)`.
- The stake target cap is the largest unsigned 256-bit target, above the
  network difficulty ceiling, so stake can help even while base difficulty is
  still at its launch setting.
- Block work is computed from the effective target. A boosted block is easier
  to find and contributes proportionally less chainwork; fork choice remains
  cumulative work.
- Stake has no direct subsidy or tax reward. The block producer receives the
  full block subsidy, transaction fees, and all tax distributions. Stake's
  benefit is collective: it improves the PoW target for every miner in its race.
- Deposits and redelegations become active after one full epoch. Unbonding
  becomes withdrawable after one full Chip War epoch (3 days at the current
  protocol epoch duration). Mining stake is independent of finality stake.

## Implementation status

The current code has signed `Delegate`, `Undelegate`, and `Withdraw`
transaction actions; same-race and nonce checks; escrow accounting; one-epoch
activation and one-epoch unbonding tied to completed Chip War epochs; state-root
inclusion; and rollback journals. The chain spec commits to stake parameters.

Wallet CLI submission (`wallet stake delegate|undelegate|withdraw`), the
`/stake/{address}` RPC view, and explorer transaction labels are connected.
Mining derives the same easier target for every miner in a race from the race
pool's active stake in the parent state, and cumulative work is calculated from
that effective target.
Header synchronization treats peer work as provisional and compares branches
only after downloading bodies and validating them against staged ledger state.
Stake is voluntary and has no direct financial return. Its active amount only
supports race-wide PoW; the miner still receives the full block subsidy and
all taxes and fees.

## Required consensus work

Stake-aware target and work must remain consistent across full block
validation, header synchronization, cumulative-work accounting, fork choice,
reorganization, and mining candidate construction. This implementation follows
the body-first validation option: header sync does not verify PoW or trust peer
work; it downloads block bodies, validates each branch transition, then
compares locally derived cumulative work.

Stake transitions are reflected in mempool validation and wallet/RPC tooling.

## Activation and compatibility

The chain-spec identity must commit to all stake rules and constants. Existing
nodes must reject peers with a different chain spec. Existing ledgers and
wallets must not silently migrate; a coordinated activation or fresh network
is required. The current transaction format, block format, chain-spec digest,
and genesis identity need explicit compatibility decisions before activation.

## Open review points

1. Whether 100,000 race coins is a sensible full-boost reference for all three
   races.
2. Whether a 2x maximum target boost is appropriate for all races.
3. Whether voluntary, unrewarded stake is sufficient incentive for users.
4. Whether body-first synchronization cost is acceptable for long fork candidates.
5. Whether a new genesis is acceptable, or a precisely defined activation
   height is required.
