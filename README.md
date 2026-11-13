# pinocchio-amm

A constant-product AMM (x · y = k, Uniswap v2 style) on Solana, written in Rust with
[Pinocchio](https://github.com/anza-xyz/pinocchio) and no Anchor. Every account is
validated by hand, pool state is read zero-copy, and the program is `no_std` with no heap.

|  | |
|---|---|
| Binary size | 43 KB (an equivalent Anchor program is several hundred KB) |
| `swap` | ≈ 13.3k compute units, most of it the two SPL Token transfers |
| Dependencies | `pinocchio`, `pinocchio-token`, `pinocchio-system` |
| Tests | 26 (7 unit, 19 LiteSVM integration) |

## Instructions

The first byte of instruction data is the discriminator; integers are little-endian.

| # | Instruction | Data | What |
|---|---|---|---|
| 0 | `initialize_pool` | `fee_bps: u16` | Creates the pool PDA, two PDA vaults and a PDA LP mint for a sorted mint pair |
| 1 | `add_liquidity` | `max_a, max_b, min_lp: u64` | Mints LP for the scarcer side, pulls the ratio-exact amounts (rounded up) |
| 2 | `remove_liquidity` | `lp, min_a, min_b: u64` | Burns LP, returns the pro-rata reserves (rounded down) |
| 3 | `swap` | `amount_in, min_out: u64, a_to_b: u8` | Exact-input swap with the fee taken from the input |

Account lists are documented above each handler in [`src/instructions/`](src/instructions/).

## Design

- **Manual account validation.** Each handler destructures `&[AccountView]` with a slice
  pattern and checks signer, program ownership, size, discriminator, PDA addresses and the
  token program ID explicitly.
- **Zero-copy state.** [`Pool`](src/state.rs) is `#[repr(C)]` with only byte-array fields, so
  its alignment is 1 (enforced by a compile-time assert) and it can be cast straight from
  account data. Accessors convert the little-endian integers.
- **Tracked reserves.** Reserves are stored in pool state, not read from vault balances, so
  tokens sent directly to a vault cannot move the price.
- **Locked minimum liquidity.** The first deposit locks 1,000 LP units (as in Uniswap v2).
  Rather than minting them to a dead address, the effective supply is `mint supply + 1000`,
  which keeps reserves non-zero forever and makes first-depositor manipulation uneconomic.
- **Rounding favours the pool.** Payouts round down, amounts taken in round up. `swap_out`
  can never return the whole reserve, and `swap` also re-checks that `x · y` did not shrink.
- **Canonical pool per pair.** Mints must be passed in ascending order (this also rejects
  identical mints), and canonical bumps are found once at init and stored.
- **Borrow-safe CPIs.** Pool state is mutated inside a scope, and the PDA seeds are copied out
  (`PoolSigner`) before any CPI that passes the pool account as a signer.
- **Error codes offset by 6000** so they never collide with SPL Token errors that bubble up
  from a failed CPI.

The pricing maths in [`math.rs`](src/math.rs) is pure and unit-tested: integer sqrt,
mul-div with explicit rounding, swap output, deposit and withdraw amounts.

## Tests

Integration tests run the compiled program in [LiteSVM](https://github.com/LiteSVM/litesvm):

- Initialization: account layout, unsorted/duplicate mints, fee cap, double init.
- Liquidity: locked minimum on first deposit, proportional deposits, withdraw-everything,
  fees accruing to LPs, slippage bounds.
- Swaps: exact constant-product output, round trip loses to fees, slippage, and a 120-swap
  randomized run asserting `k` never decreases and the vaults always cover the reserves.
- Attacks: donation to a vault, fake token program, own account passed as the input vault
  (drain attempt), forged pool account owned by another program, missing signer, burning
  someone else's LP, wrong-mint destination.

The security tests were checked by mutation: removing the token program check, the input
vault check or the output vault check each makes its test fail.

## Build and test

Toolchain: Rust (stable), Solana CLI 2.3 (Agave).

```bash
cargo build-sbf
cargo test
```

Dependencies are pinned to versions that support rustc 1.84, the compiler in the platform
tools shipped with Solana 2.3 (`rust-version = "1.84"` plus Cargo's MSRV-aware resolver in
`.cargo/config.toml`).

See [SECURITY.md](SECURITY.md) for the threat model. Not audited.
