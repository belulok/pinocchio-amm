use pinocchio::{error::ProgramError, AccountView};

use crate::error::AmmError;

#[inline(always)]
pub fn require_signer(a: &AccountView) -> Result<(), ProgramError> {
    if a.is_signer() {
        Ok(())
    } else {
        Err(ProgramError::MissingRequiredSignature)
    }
}

/// The token program account must be the real SPL Token program, or a malicious program
/// could be invoked with the pool PDA's signature.
#[inline(always)]
pub fn require_token_program(a: &AccountView) -> Result<(), ProgramError> {
    if a.address() == &pinocchio_token::ID {
        Ok(())
    } else {
        Err(ProgramError::IncorrectProgramId)
    }
}

#[inline(always)]
pub fn require_key(a: &AccountView, expected: &[u8; 32]) -> Result<(), ProgramError> {
    if a.address().as_ref() == expected {
        Ok(())
    } else {
        Err(AmmError::AccountMismatch.into())
    }
}

#[inline(always)]
pub fn read_u64(data: &[u8], at: usize) -> Result<u64, ProgramError> {
    data.get(at..at + 8)
        .map(|b| u64::from_le_bytes(b.try_into().unwrap()))
        .ok_or(ProgramError::InvalidInstructionData)
}
