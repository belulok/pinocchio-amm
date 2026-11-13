# Security notes

Without Anchor, nothing is checked unless the program checks it. This is the full list of
what `pinocchio-amm` validates, why, and the test that covers it.

| Threat | Check | Test |
|---|---|---|
| **Forged pool account** with fake reserves | Pool must be owned by this program, exactly 184 bytes, discriminator 1 | `forged_pool_account_is_rejected` |
| **Arbitrary CPI target.** A fake "token program" would receive the pool PDA's signature | Token program account must equal the SPL Token program ID; system program checked at init | `fake_token_program_is_rejected` |
| **Vault substitution / drain.** Passing your own token account as the input vault so you pay yourself while the real output vault pays you | Both vault addresses must equal those stored in pool state | `substituted_input_vault_is_rejected` |
| **Wrong LP mint** | LP mint address pinned in pool state | covered by the liquidity tests |
| **Duplicate pools / same-mint pool** | Mints strictly sorted; canonical bump from `find_program_address`; account creation fails if the pool exists | `initialize_rejects_unsorted_or_duplicate_mints`, `initialize_rejects_excessive_fee_and_double_init` |
| **Acting for another user** | User must be a signer; token transfers and burns from user accounts require that signature | `user_must_sign`, `cannot_burn_someone_elses_lp` |
| **Mixed mints** | Vaults are fixed to the pool's mints; SPL Token rejects transfers between different mints | `wrong_mint_destination_fails_in_token_program` |
| **Donation / price manipulation via vault balance** | Reserves tracked in state; vault balances are never read for pricing | `donation_does_not_move_the_price` |
| **First-depositor manipulation** | 1,000 LP units locked forever (virtual supply), initial LP = `sqrt(a·b)` | `first_deposit_locks_minimum_liquidity` |
| **Rounding extraction** | Payouts floor, deposits ceil; `swap_out < reserve_out`; `x·y` re-checked after every swap | `deposit_then_withdraw_never_profits`, `k_never_decreases_over_random_swaps` |
| **Overflow** | u128 intermediates, `checked_*` on reserves, `overflow-checks = true` in release | `swap_never_drains_pool`, `isqrt_is_exact_floor` |
| **Sandwiching** | `min_out`, `min_lp`, `min_a`/`min_b` slippage bounds | `swap_slippage_bound`, `deposit_and_withdraw_slippage_bounds` |
| **Unsafe zero-copy cast** | `Pool` has alignment 1 (compile-time assert) and only byte-array fields, so any bit pattern is valid; length checked before casting | `initialize_creates_pool_vaults_and_lp_mint` |
| **Error ambiguity** | Custom errors start at 6000 so they never collide with SPL Token's | asserted throughout the tests |

## Known limitations

- **Pre-funded PDA griefing.** `CreateAccount` fails if someone sends lamports to the pool,
  vault or LP mint address before the pool exists, blocking that pair. A hardened version
  would transfer/allocate/assign instead of `CreateAccount`.
- **SPL Token only.** Token-2022 is rejected by the token program ID check (transfer fees and
  hooks would need explicit handling).
- **No admin and no protocol fee.** The fee is fixed at creation and goes entirely to LPs.
- **Spot-price only.** There is no TWAP oracle; do not use the reserves as a price feed.
- **Not audited.**
