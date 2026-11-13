//! Pricing maths. Every rounding choice favours the pool (and therefore existing LPs):
//! amounts paid out round down, amounts taken in round up.

use crate::error::AmmError;

pub const BPS: u64 = 10_000;
/// LP units permanently locked on first deposit (as in Uniswap v2). Instead of minting
/// them to a dead address they are added virtually: effective supply = mint supply + this.
pub const MINIMUM_LIQUIDITY: u64 = 1_000;

type R<T> = Result<T, AmmError>;

pub fn mul_div_floor(a: u64, b: u64, c: u64) -> R<u64> {
    if c == 0 {
        return Err(AmmError::MathOverflow);
    }
    u64::try_from(a as u128 * b as u128 / c as u128).map_err(|_| AmmError::MathOverflow)
}

pub fn mul_div_ceil(a: u64, b: u64, c: u64) -> R<u64> {
    if c == 0 {
        return Err(AmmError::MathOverflow);
    }
    let p = a as u128 * b as u128;
    u64::try_from(p.div_ceil(c as u128)).map_err(|_| AmmError::MathOverflow)
}

/// Integer square root, rounded down (Newton's method on u128).
pub fn isqrt(n: u128) -> u128 {
    if n < 2 {
        return n;
    }
    let mut x = n;
    let mut y = (x >> 1) + (x & 1); // ceil(x / 2) without overflowing at u128::MAX
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Output of an exact-input swap. The fee is taken from the input and stays in the pool.
pub fn swap_out(amount_in: u64, reserve_in: u64, reserve_out: u64, fee_bps: u16) -> R<u64> {
    if amount_in == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if reserve_in == 0 || reserve_out == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    let in_after_fee = amount_in as u128 * (BPS - fee_bps as u64) as u128;
    let num = in_after_fee * reserve_out as u128;
    let den = reserve_in as u128 * BPS as u128 + in_after_fee;
    // num / den < reserve_out, so the pool can never be fully drained.
    u64::try_from(num / den).map_err(|_| AmmError::MathOverflow)
}

/// LP to mint and the exact amounts to pull for a deposit of up to (`max_a`, `max_b`).
/// `supply` is the effective supply (including locked liquidity).
pub fn deposit_amounts(max_a: u64, max_b: u64, reserve_a: u64, reserve_b: u64, supply: u64) -> R<Deposit> {
    if max_a == 0 || max_b == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if supply == 0 {
        // First deposit: LP = sqrt(a·b), of which MINIMUM_LIQUIDITY is locked forever.
        let root = isqrt(max_a as u128 * max_b as u128) as u64;
        if root <= MINIMUM_LIQUIDITY {
            return Err(AmmError::InitialLiquidityTooLow);
        }
        return Ok(Deposit {
            lp: root - MINIMUM_LIQUIDITY,
            amount_a: max_a,
            amount_b: max_b,
            new_supply: root,
        });
    }
    // Mint for the scarcer side, then charge the ratio-exact amounts rounded up.
    let lp = mul_div_floor(max_a, supply, reserve_a)?.min(mul_div_floor(max_b, supply, reserve_b)?);
    if lp == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let amount_a = mul_div_ceil(lp, reserve_a, supply)?;
    let amount_b = mul_div_ceil(lp, reserve_b, supply)?;
    let new_supply = supply.checked_add(lp).ok_or(AmmError::MathOverflow)?;
    Ok(Deposit { lp, amount_a, amount_b, new_supply })
}

#[derive(Debug, PartialEq, Eq)]
pub struct Deposit {
    pub lp: u64,
    pub amount_a: u64,
    pub amount_b: u64,
    /// Effective supply after the deposit (mint supply + locked).
    pub new_supply: u64,
}

/// Amounts returned for burning `lp`, rounded down.
pub fn withdraw_amounts(lp: u64, reserve_a: u64, reserve_b: u64, supply: u64) -> R<(u64, u64)> {
    if lp == 0 {
        return Err(AmmError::ZeroAmount);
    }
    Ok((mul_div_floor(lp, reserve_a, supply)?, mul_div_floor(lp, reserve_b, supply)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isqrt_is_exact_floor() {
        for n in [
            0u128,
            1,
            2,
            3,
            4,
            15,
            16,
            17,
            1 << 40,
            (1u128 << 64) - 1,
            u64::MAX as u128 * u64::MAX as u128,
            u128::MAX,
        ] {
            let r = isqrt(n);
            let next = (r + 1).checked_mul(r + 1);
            assert!(r * r <= n && next.is_none_or(|x| x > n), "isqrt({n}) = {r}");
        }
    }

    #[test]
    fn swap_matches_constant_product_and_k_grows() {
        let (ra, rb) = (1_000_000u64, 2_000_000u64);
        let out = swap_out(10_000, ra, rb, 30).unwrap();
        // Without fee: 2_000_000 * 10_000 / 1_010_000 = 19_801. With 0.3% fee:
        // 9_970 * 10_000 * 2_000_000 / (1_000_000 * 10_000 + 9_970 * 10_000) = 19_743.16
        assert_eq!(out, 19_743);
        let k_before = ra as u128 * rb as u128;
        let k_after = (ra + 10_000) as u128 * (rb - out) as u128;
        assert!(k_after > k_before);
    }

    #[test]
    fn swap_never_drains_pool() {
        let out = swap_out(u64::MAX, 1_000, 1_000, 0).unwrap();
        assert!(out < 1_000);
    }

    #[test]
    fn first_deposit_locks_minimum_liquidity() {
        let d = deposit_amounts(4_000_000, 1_000_000, 0, 0, 0).unwrap();
        assert_eq!(d.new_supply, 2_000_000);
        assert_eq!(d.lp, 2_000_000 - MINIMUM_LIQUIDITY);
        assert_eq!(deposit_amounts(1_000, 1_000, 0, 0, 0), Err(AmmError::InitialLiquidityTooLow));
    }

    #[test]
    fn proportional_deposit_rounds_against_depositor() {
        // Pool 1000:3000 with supply 1000 (ratio 1:3). Offer 101 A and plenty of B.
        let d = deposit_amounts(101, 1_000_000, 1_000, 3_000, 1_000).unwrap();
        assert_eq!(d.lp, 101);
        assert_eq!((d.amount_a, d.amount_b), (101, 303));
        // Odd ratio: amounts round up.
        let d = deposit_amounts(10, 10, 3, 7, 5).unwrap();
        assert_eq!(d.lp, 7); // min(10*5/3=16, 10*5/7=7)
        assert_eq!((d.amount_a, d.amount_b), (5, 10)); // ceil(7*3/5)=5, ceil(7*7/5)=10
    }

    #[test]
    fn deposit_then_withdraw_never_profits() {
        let (ra, rb, s) = (1_234_567u64, 7_654_321u64, 3_000_000u64);
        for (a, b) in [(1u64, 1u64), (999, 5_000), (123_456, 1_000_000), (7, 3)] {
            let Ok(d) = deposit_amounts(a, b, ra, rb, s) else { continue };
            let (wa, wb) = withdraw_amounts(d.lp, ra + d.amount_a, rb + d.amount_b, d.new_supply).unwrap();
            assert!(wa <= d.amount_a && wb <= d.amount_b, "round trip gained: {d:?} -> {wa},{wb}");
        }
    }
}
