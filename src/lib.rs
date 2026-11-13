//! Constant-product AMM (x · y = k) written with Pinocchio: no Anchor, no heap, no std.
//!
//! Every account is validated by hand in [`instructions`]; pool state is read zero-copy
//! from account data in [`state`]; all pricing maths lives in [`math`] as pure functions.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod error;
pub mod instructions;
pub mod math;
pub mod state;
mod utils;

use pinocchio::{address::declare_id, error::ProgramError, AccountView, Address, ProgramResult};

declare_id!("HnJicxmboMj74w3KV8WZqug4TKf2n9uUsoAyt6rtGrWb");

#[cfg(not(feature = "no-entrypoint"))]
mod entrypoint {
    use pinocchio::{no_allocator, nostd_panic_handler, program_entrypoint};
    program_entrypoint!(crate::process_instruction);
    no_allocator!();
    nostd_panic_handler!();
}

/// Instruction discriminators (first byte of instruction data).
pub mod ix {
    pub const INITIALIZE_POOL: u8 = 0;
    pub const ADD_LIQUIDITY: u8 = 1;
    pub const REMOVE_LIQUIDITY: u8 = 2;
    pub const SWAP: u8 = 3;
}

pub fn process_instruction(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let (tag, args) = data.split_first().ok_or(ProgramError::InvalidInstructionData)?;
    match *tag {
        ix::INITIALIZE_POOL => instructions::initialize_pool(program_id, accounts, args),
        ix::ADD_LIQUIDITY => instructions::add_liquidity(program_id, accounts, args),
        ix::REMOVE_LIQUIDITY => instructions::remove_liquidity(program_id, accounts, args),
        ix::SWAP => instructions::swap(program_id, accounts, args),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}
