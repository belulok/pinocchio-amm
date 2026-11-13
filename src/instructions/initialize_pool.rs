use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    sysvars::{rent::Rent, Sysvar},
    AccountView, Address, ProgramResult,
};
use pinocchio_system::instructions::CreateAccount;
use pinocchio_token::{
    instructions::{InitializeAccount3, InitializeMint2},
    state::{Mint, TokenAccount},
};

use crate::{
    error::AmmError,
    state::*,
    utils::{require_signer, require_token_program},
};

/// Accounts:
///   0. `[signer, writable]` payer
///   1. `[writable]` pool        PDA ["pool", mint_a, mint_b]
///   2. `[]`         mint_a      must sort below mint_b
///   3. `[]`         mint_b
///   4. `[writable]` vault_a     PDA ["vault", pool, mint_a]
///   5. `[writable]` vault_b     PDA ["vault", pool, mint_b]
///   6. `[writable]` lp_mint     PDA ["lp", pool]
///   7. `[]`         system program
///   8. `[]`         token program
///
/// Data: fee_bps (u16 LE)
pub fn initialize_pool(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [payer, pool, mint_a, mint_b, vault_a, vault_b, lp_mint, system_program, token_program, ..] =
        accounts
    else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let fee_bps =
        u16::from_le_bytes(data.get(..2).ok_or(ProgramError::InvalidInstructionData)?.try_into().unwrap());

    require_signer(payer)?;
    require_token_program(token_program)?;
    if system_program.address() != &pinocchio_system::ID {
        return Err(ProgramError::IncorrectProgramId);
    }
    if fee_bps > MAX_FEE_BPS {
        return Err(AmmError::FeeTooHigh.into());
    }
    // One canonical pool per pair: strict ordering also rejects mint_a == mint_b.
    if mint_a.address().as_ref() >= mint_b.address().as_ref() {
        return Err(AmmError::MintsNotSorted.into());
    }
    // Owner + length checks: both must be real SPL Token mints.
    drop(Mint::from_account_view(mint_a)?);
    drop(Mint::from_account_view(mint_b)?);

    // Canonical bumps, so the addresses are unique per pair.
    let (pool_key, bump) = Address::find_program_address(
        &[POOL_SEED, mint_a.address().as_ref(), mint_b.address().as_ref()],
        program_id,
    );
    if pool.address() != &pool_key {
        return Err(AmmError::AccountMismatch.into());
    }
    let (va_key, va_bump) = Address::find_program_address(
        &[VAULT_SEED, pool_key.as_ref(), mint_a.address().as_ref()],
        program_id,
    );
    let (vb_key, vb_bump) = Address::find_program_address(
        &[VAULT_SEED, pool_key.as_ref(), mint_b.address().as_ref()],
        program_id,
    );
    let (lp_key, lp_bump) = Address::find_program_address(&[LP_SEED, pool_key.as_ref()], program_id);
    if vault_a.address() != &va_key || vault_b.address() != &vb_key || lp_mint.address() != &lp_key {
        return Err(AmmError::AccountMismatch.into());
    }

    let rent = Rent::get()?;

    // Pool state account.
    let bump_b = [bump];
    let pool_seeds = [
        Seed::from(POOL_SEED),
        Seed::from(mint_a.address().as_ref()),
        Seed::from(mint_b.address().as_ref()),
        Seed::from(&bump_b),
    ];
    CreateAccount {
        from: payer,
        to: pool,
        lamports: rent.try_minimum_balance(Pool::LEN)?,
        space: Pool::LEN as u64,
        owner: program_id,
    }
    .invoke_signed(&[Signer::from(&pool_seeds)])?;

    // Vaults: token accounts at PDAs, owned (as token authority) by the pool PDA.
    for (vault, mint, vbump) in [(vault_a, mint_a, va_bump), (vault_b, mint_b, vb_bump)] {
        let b = [vbump];
        let seeds = [
            Seed::from(VAULT_SEED),
            Seed::from(pool_key.as_ref()),
            Seed::from(mint.address().as_ref()),
            Seed::from(&b),
        ];
        CreateAccount {
            from: payer,
            to: vault,
            lamports: rent.try_minimum_balance(TokenAccount::LEN)?,
            space: TokenAccount::LEN as u64,
            owner: &pinocchio_token::ID,
        }
        .invoke_signed(&[Signer::from(&seeds)])?;
        InitializeAccount3 { account: vault, mint, owner: &pool_key }.invoke()?;
    }

    // LP mint: the pool PDA is the only mint authority, no freeze authority.
    let lb = [lp_bump];
    let lp_seeds = [Seed::from(LP_SEED), Seed::from(pool_key.as_ref()), Seed::from(&lb)];
    CreateAccount {
        from: payer,
        to: lp_mint,
        lamports: rent.try_minimum_balance(Mint::LEN)?,
        space: Mint::LEN as u64,
        owner: &pinocchio_token::ID,
    }
    .invoke_signed(&[Signer::from(&lp_seeds)])?;
    InitializeMint2 {
        mint: lp_mint,
        decimals: LP_DECIMALS,
        mint_authority: &pool_key,
        freeze_authority: None,
    }
    .invoke()?;

    let mut p = Pool::init(pool)?;
    p.discriminator = POOL_DISCRIMINATOR;
    p.bump = bump;
    p.vault_a_bump = va_bump;
    p.vault_b_bump = vb_bump;
    p.lp_mint_bump = lp_bump;
    p.set_fee_bps(fee_bps);
    p.mint_a.copy_from_slice(mint_a.address().as_ref());
    p.mint_b.copy_from_slice(mint_b.address().as_ref());
    p.vault_a.copy_from_slice(va_key.as_ref());
    p.vault_b.copy_from_slice(vb_key.as_ref());
    p.lp_mint.copy_from_slice(lp_key.as_ref());
    p.set_reserves(0, 0);
    Ok(())
}
