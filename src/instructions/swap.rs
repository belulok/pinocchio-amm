use pinocchio::{cpi::Signer, error::ProgramError, AccountView, Address, ProgramResult};
use pinocchio_token::instructions::Transfer;

use crate::{
    error::AmmError,
    math::swap_out,
    state::{Pool, PoolSigner},
    utils::{read_u64, require_key, require_signer, require_token_program},
};

/// Exact-input swap.
///
/// Accounts:
///   0. `[signer]`   user
///   1. `[writable]` pool
///   2. `[writable]` vault_a
///   3. `[writable]` vault_b
///   4. `[writable]` user source token account (mint of the input side)
///   5. `[writable]` user destination token account (mint of the output side)
///   6. `[]`         token program
///
/// Data: amount_in u64, min_out u64, a_to_b u8 (1 = A→B, 0 = B→A)
///
/// Source/destination mints are not checked here: the token program rejects a transfer
/// between accounts of different mints, and the vaults are pinned to the pool's mints.
pub fn swap(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [user, pool, vault_a, vault_b, user_src, user_dst, token_program, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let (amount_in, min_out) = (read_u64(data, 0)?, read_u64(data, 8)?);
    let a_to_b = match data.get(16) {
        Some(1) => true,
        Some(0) => false,
        _ => return Err(ProgramError::InvalidInstructionData),
    };
    require_signer(user)?;
    require_token_program(token_program)?;

    let (signer, out) = {
        let mut p = Pool::load_mut(pool, program_id)?;
        require_key(vault_a, &p.vault_a)?;
        require_key(vault_b, &p.vault_b)?;

        let (ra, rb) = (p.reserve_a(), p.reserve_b());
        let (r_in, r_out) = if a_to_b { (ra, rb) } else { (rb, ra) };
        let out = swap_out(amount_in, r_in, r_out, p.fee_bps())?;
        if out == 0 {
            return Err(AmmError::ZeroAmount.into());
        }
        if out < min_out {
            return Err(AmmError::SlippageExceeded.into());
        }

        let new_in = r_in.checked_add(amount_in).ok_or(AmmError::MathOverflow)?;
        let new_out = r_out - out; // out < r_out by construction of swap_out
                                   // Defence in depth: the product of reserves must never shrink.
        if (new_in as u128) * (new_out as u128) < (r_in as u128) * (r_out as u128) {
            return Err(AmmError::InvariantViolated.into());
        }
        if a_to_b {
            p.set_reserves(new_in, new_out)
        } else {
            p.set_reserves(new_out, new_in)
        }
        (PoolSigner::new(&p), out)
    };

    let (vault_in, vault_out) = if a_to_b { (vault_a, vault_b) } else { (vault_b, vault_a) };
    Transfer { from: user_src, to: vault_in, authority: user, amount: amount_in }.invoke()?;
    let seeds = signer.seeds();
    Transfer { from: vault_out, to: user_dst, authority: pool, amount: out }
        .invoke_signed(&[Signer::from(&seeds)])
}
