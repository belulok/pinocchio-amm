use pinocchio::error::ProgramError;

/// Offset so our codes never collide with SPL Token's (0..=19), which surface unchanged when a CPI fails.
pub const ERROR_BASE: u32 = 6000;

/// Custom errors, surfaced as `ProgramError::Custom(ERROR_BASE + code)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum AmmError {
    /// Mints must be passed in ascending address order and be different.
    MintsNotSorted = 0,
    FeeTooHigh = 1,
    /// A passed account does not match the address stored in, or derived for, the pool.
    AccountMismatch = 2,
    ZeroAmount = 3,
    SlippageExceeded = 4,
    InsufficientLiquidity = 5,
    MathOverflow = 6,
    /// Swap would break x·y >= k. Unreachable with correct maths; checked anyway.
    InvariantViolated = 7,
    InitialLiquidityTooLow = 8,
}

impl From<AmmError> for ProgramError {
    fn from(e: AmmError) -> Self {
        ProgramError::Custom(ERROR_BASE + e as u32)
    }
}
