//! Pool account, read in place from account data (zero-copy).
//!
//! Every field is a byte array, so the struct has alignment 1 and can be cast from any
//! account data pointer without alignment UB.

use pinocchio::{
    account::{Ref, RefMut},
    error::ProgramError,
    AccountView, Address,
};

pub const POOL_SEED: &[u8] = b"pool";
pub const VAULT_SEED: &[u8] = b"vault";
pub const LP_SEED: &[u8] = b"lp";

pub const POOL_DISCRIMINATOR: u8 = 1;
pub const MAX_FEE_BPS: u16 = 1_000; // 10%
pub const LP_DECIMALS: u8 = 9;

#[repr(C)]
pub struct Pool {
    pub discriminator: u8,
    pub bump: u8,
    pub vault_a_bump: u8,
    pub vault_b_bump: u8,
    pub lp_mint_bump: u8,
    fee_bps: [u8; 2],
    _padding: u8,
    pub mint_a: [u8; 32],
    pub mint_b: [u8; 32],
    pub vault_a: [u8; 32],
    pub vault_b: [u8; 32],
    pub lp_mint: [u8; 32],
    /// Reserves are tracked here, not read from the vault balances, so tokens sent
    /// straight to a vault cannot move the price.
    reserve_a: [u8; 8],
    reserve_b: [u8; 8],
}

const _: () = assert!(core::mem::align_of::<Pool>() == 1);

impl Pool {
    pub const LEN: usize = core::mem::size_of::<Pool>();

    pub fn fee_bps(&self) -> u16 {
        u16::from_le_bytes(self.fee_bps)
    }
    pub fn set_fee_bps(&mut self, v: u16) {
        self.fee_bps = v.to_le_bytes();
    }
    pub fn reserve_a(&self) -> u64 {
        u64::from_le_bytes(self.reserve_a)
    }
    pub fn reserve_b(&self) -> u64 {
        u64::from_le_bytes(self.reserve_b)
    }
    pub fn set_reserves(&mut self, a: u64, b: u64) {
        self.reserve_a = a.to_le_bytes();
        self.reserve_b = b.to_le_bytes();
    }

    /// Owner, size and discriminator checks, then a shared zero-copy borrow.
    pub fn load<'a>(account: &'a AccountView, program_id: &Address) -> Result<Ref<'a, Pool>, ProgramError> {
        Self::check(account, program_id)?;
        // SAFETY: length checked; Pool has alignment 1 and no invalid bit patterns.
        Ok(Ref::map(account.try_borrow()?, |d| unsafe { &*(d.as_ptr() as *const Pool) }))
    }

    pub fn load_mut<'a>(
        account: &'a AccountView,
        program_id: &Address,
    ) -> Result<RefMut<'a, Pool>, ProgramError> {
        Self::check(account, program_id)?;
        if !account.is_writable() {
            return Err(ProgramError::InvalidAccountData);
        }
        // SAFETY: as above.
        Ok(RefMut::map(account.try_borrow_mut()?, |d| unsafe { &mut *(d.as_mut_ptr() as *mut Pool) }))
    }

    /// Uninitialised view used once, right after the account is created.
    pub fn init<'a>(account: &'a AccountView) -> Result<RefMut<'a, Pool>, ProgramError> {
        if account.data_len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(RefMut::map(account.try_borrow_mut()?, |d| unsafe { &mut *(d.as_mut_ptr() as *mut Pool) }))
    }

    fn check(account: &AccountView, program_id: &Address) -> Result<(), ProgramError> {
        if !account.owned_by(program_id) {
            return Err(ProgramError::InvalidAccountOwner);
        }
        if account.data_len() != Self::LEN {
            return Err(ProgramError::InvalidAccountData);
        }
        // SAFETY: owned by this program and the right length; read-only peek at byte 0.
        if unsafe { account.borrow_unchecked() }[0] != POOL_DISCRIMINATOR {
            return Err(ProgramError::InvalidAccountData);
        }
        Ok(())
    }
}

/// Owned copy of the pool PDA seeds so the pool account can be borrowed freely around CPIs.
pub struct PoolSigner {
    mint_a: [u8; 32],
    mint_b: [u8; 32],
    bump: [u8; 1],
}

impl PoolSigner {
    pub fn new(p: &Pool) -> Self {
        Self { mint_a: p.mint_a, mint_b: p.mint_b, bump: [p.bump] }
    }

    pub fn seeds(&self) -> [pinocchio::cpi::Seed<'_>; 4] {
        use pinocchio::cpi::Seed;
        [Seed::from(POOL_SEED), Seed::from(&self.mint_a), Seed::from(&self.mint_b), Seed::from(&self.bump)]
    }
}
