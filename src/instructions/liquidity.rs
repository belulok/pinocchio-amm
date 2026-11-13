use pinocchio::{cpi::Signer, error::ProgramError, AccountView, Address, ProgramResult};
use pinocchio_token::{
    instructions::{Burn, MintTo, Transfer},
    state::Mint,
};

use crate::{
    error::AmmError,
    math::{deposit_amounts, withdraw_amounts, MINIMUM_LIQUIDITY},
    state::{Pool, PoolSigner},
    utils::{read_u64, require_key, require_signer, require_token_program},
};

/// Accounts (both instructions):
///   0. `[signer]`   user
///   1. `[writable]` pool
///   2. `[writable]` vault_a
///   3. `[writable]` vault_b
///   4. `[writable]` lp_mint
///   5. `[writable]` user token account for mint_a
///   6. `[writable]` user token account for mint_b
///   7. `[writable]` user LP token account
///   8. `[]`         token program
///
/// add_liquidity data:    max_a u64, max_b u64, min_lp u64
/// remove_liquidity data: lp u64, min_a u64, min_b u64
pub fn add_liquidity(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [user, pool, vault_a, vault_b, lp_mint, user_a, user_b, user_lp, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (max_a, max_b, min_lp) = (read_u64(data, 0)?, read_u64(data, 8)?, read_u64(data, 16)?);
    require_signer(user)?;
    require_token_program(token_program)?;

    let (signer, d) = {
        let mut p = Pool::load_mut(pool, program_id)?;
        check_pool_accounts(&p, vault_a, vault_b, lp_mint)?;
        let (ra, rb) = (p.reserve_a(), p.reserve_b());
        let supply = Mint::from_account_view(lp_mint)?.supply();
        // Reserves are zero only before the first deposit; afterwards the locked
        // MINIMUM_LIQUIDITY keeps them non-zero forever.
        let effective_supply = if ra == 0 { 0 } else { supply + MINIMUM_LIQUIDITY };

        let d = deposit_amounts(max_a, max_b, ra, rb, effective_supply)?;
        if d.lp < min_lp {
            return Err(AmmError::SlippageExceeded.into());
        }
        p.set_reserves(
            ra.checked_add(d.amount_a).ok_or(AmmError::MathOverflow)?,
            rb.checked_add(d.amount_b).ok_or(AmmError::MathOverflow)?,
        );
        (PoolSigner::new(&p), d)
    };

    Transfer { from: user_a, to: vault_a, authority: user, amount: d.amount_a }.invoke()?;
    Transfer { from: user_b, to: vault_b, authority: user, amount: d.amount_b }.invoke()?;
    let seeds = signer.seeds();
    MintTo { mint: lp_mint, account: user_lp, mint_authority: pool, amount: d.lp }
        .invoke_signed(&[Signer::from(&seeds)])
}

pub fn remove_liquidity(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [user, pool, vault_a, vault_b, lp_mint, user_a, user_b, user_lp, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (lp, min_a, min_b) = (read_u64(data, 0)?, read_u64(data, 8)?, read_u64(data, 16)?);
    require_signer(user)?;
    require_token_program(token_program)?;

    let (signer, out_a, out_b) = {
        let mut p = Pool::load_mut(pool, program_id)?;
        check_pool_accounts(&p, vault_a, vault_b, lp_mint)?;
        let (ra, rb) = (p.reserve_a(), p.reserve_b());
        if ra == 0 {
            return Err(AmmError::InsufficientLiquidity.into());
        }
        let supply = Mint::from_account_view(lp_mint)?.supply() + MINIMUM_LIQUIDITY;

        let (out_a, out_b) = withdraw_amounts(lp, ra, rb, supply)?;
        if out_a == 0 && out_b == 0 {
            return Err(AmmError::ZeroAmount.into());
        }
        if out_a < min_a || out_b < min_b {
            return Err(AmmError::SlippageExceeded.into());
        }
        p.set_reserves(
            ra.checked_sub(out_a).ok_or(AmmError::MathOverflow)?,
            rb.checked_sub(out_b).ok_or(AmmError::MathOverflow)?,
        );
        (PoolSigner::new(&p), out_a, out_b)
    };

    // Burn needs the user's signature over their own LP account: nobody can burn yours.
    Burn { account: user_lp, mint: lp_mint, authority: user, amount: lp }.invoke()?;
    let seeds = signer.seeds();
    let signers = [Signer::from(&seeds)];
    Transfer { from: vault_a, to: user_a, authority: pool, amount: out_a }.invoke_signed(&signers)?;
    Transfer { from: vault_b, to: user_b, authority: pool, amount: out_b }.invoke_signed(&signers)
}

fn check_pool_accounts(
    p: &Pool,
    vault_a: &AccountView,
    vault_b: &AccountView,
    lp_mint: &AccountView,
) -> Result<(), ProgramError> {
    require_key(vault_a, &p.vault_a)?;
    require_key(vault_b, &p.vault_b)?;
    require_key(lp_mint, &p.lp_mint)
}
