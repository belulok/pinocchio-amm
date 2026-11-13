//! Integration tests: the compiled SBF program running inside LiteSVM.
//! Build first: `cargo build-sbf`, then `cargo test`.
#![allow(clippy::result_large_err)] // LiteSVM's error type carries the full tx logs

use litesvm::{
    types::{FailedTransactionMetadata, TransactionMetadata},
    LiteSVM,
};
use litesvm_token::{get_spl_account, spl_token, CreateAccount, CreateMint, MintTo, Transfer};
use pinocchio_amm::{
    error::{AmmError, ERROR_BASE},
    math::{swap_out, MINIMUM_LIQUIDITY},
};
use solana_account::Account;
use solana_instruction::{error::InstructionError, AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_message::Message;
use solana_pubkey::{pubkey, Pubkey};
use solana_signer::Signer;
use solana_transaction::Transaction;
use solana_transaction_error::TransactionError;

const PROGRAM_ID: Pubkey = pubkey!("HnJicxmboMj74w3KV8WZqug4TKf2n9uUsoAyt6rtGrWb");
const SYSTEM: Pubkey = pubkey!("11111111111111111111111111111111");
const FEE_BPS: u16 = 30;
const ONE: u64 = 1_000_000; // 6-decimal tokens
const POOL_LEN: usize = 184;

type TxResult = Result<TransactionMetadata, FailedTransactionMetadata>;

struct Env {
    svm: LiteSVM,
    payer: Keypair,
    mint_a: Pubkey,
    mint_b: Pubkey,
    pool: Pubkey,
    vault_a: Pubkey,
    vault_b: Pubkey,
    lp_mint: Pubkey,
}

struct User {
    kp: Keypair,
    a: Pubkey,
    b: Pubkey,
    lp: Pubkey,
}

fn pda(seeds: &[&[u8]]) -> Pubkey {
    Pubkey::find_program_address(seeds, &PROGRAM_ID).0
}

impl Env {
    /// Two fresh mints (sorted) and an uninitialised pool.
    fn bare() -> Self {
        let mut svm = LiteSVM::new();
        let so = concat!(env!("CARGO_MANIFEST_DIR"), "/target/deploy/pinocchio_amm.so");
        svm.add_program_from_file(PROGRAM_ID, so).expect("run `cargo build-sbf` first");
        let payer = Keypair::new();
        svm.airdrop(&payer.pubkey(), 1_000_000_000_000).unwrap();
        let m1 = CreateMint::new(&mut svm, &payer).decimals(6).send().unwrap();
        let m2 = CreateMint::new(&mut svm, &payer).decimals(6).send().unwrap();
        let (mint_a, mint_b) = if m1 < m2 { (m1, m2) } else { (m2, m1) };
        let pool = pda(&[b"pool", mint_a.as_ref(), mint_b.as_ref()]);
        let vault_a = pda(&[b"vault", pool.as_ref(), mint_a.as_ref()]);
        let vault_b = pda(&[b"vault", pool.as_ref(), mint_b.as_ref()]);
        let lp_mint = pda(&[b"lp", pool.as_ref()]);
        Env { svm, payer, mint_a, mint_b, pool, vault_a, vault_b, lp_mint }
    }

    fn new() -> Self {
        let mut env = Self::bare();
        let ix = env.init_ix(env.mint_a, env.mint_b, FEE_BPS);
        env.send_payer(ix).unwrap();
        env
    }

    fn send(&mut self, ix: Instruction, signer: &Keypair) -> TxResult {
        self.svm.expire_blockhash();
        let tx = Transaction::new(
            &[signer],
            Message::new(&[ix], Some(&signer.pubkey())),
            self.svm.latest_blockhash(),
        );
        self.svm.send_transaction(tx)
    }

    fn send_payer(&mut self, ix: Instruction) -> TxResult {
        let payer = self.payer.insecure_clone();
        self.send(ix, &payer)
    }

    fn init_ix(&self, mint_a: Pubkey, mint_b: Pubkey, fee_bps: u16) -> Instruction {
        let pool = pda(&[b"pool", mint_a.as_ref(), mint_b.as_ref()]);
        let mut data = vec![0u8];
        data.extend_from_slice(&fee_bps.to_le_bytes());
        Instruction {
            program_id: PROGRAM_ID,
            accounts: vec![
                AccountMeta::new(self.payer.pubkey(), true),
                AccountMeta::new(pool, false),
                AccountMeta::new_readonly(mint_a, false),
                AccountMeta::new_readonly(mint_b, false),
                AccountMeta::new(pda(&[b"vault", pool.as_ref(), mint_a.as_ref()]), false),
                AccountMeta::new(pda(&[b"vault", pool.as_ref(), mint_b.as_ref()]), false),
                AccountMeta::new(pda(&[b"lp", pool.as_ref()]), false),
                AccountMeta::new_readonly(SYSTEM, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data,
        }
    }

    fn user(&mut self, a: u64, b: u64) -> User {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 10_000_000_000).unwrap();
        let (ma, mb, ml) = (self.mint_a, self.mint_b, self.lp_mint);
        let ta = CreateAccount::new(&mut self.svm, &kp, &ma).owner(&kp.pubkey()).send().unwrap();
        let tb = CreateAccount::new(&mut self.svm, &kp, &mb).owner(&kp.pubkey()).send().unwrap();
        let tl = CreateAccount::new(&mut self.svm, &kp, &ml).owner(&kp.pubkey()).send().unwrap();
        if a > 0 {
            MintTo::new(&mut self.svm, &self.payer, &ma, &ta, a).send().unwrap();
        }
        if b > 0 {
            MintTo::new(&mut self.svm, &self.payer, &mb, &tb, b).send().unwrap();
        }
        User { kp, a: ta, b: tb, lp: tl }
    }

    fn liquidity_ix(&self, tag: u8, u: &User, x: u64, y: u64, z: u64) -> Instruction {
        let mut data = vec![tag];
        for v in [x, y, z] {
            data.extend_from_slice(&v.to_le_bytes());
        }
        Instruction {
            program_id: PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(u.kp.pubkey(), true),
                AccountMeta::new(self.pool, false),
                AccountMeta::new(self.vault_a, false),
                AccountMeta::new(self.vault_b, false),
                AccountMeta::new(self.lp_mint, false),
                AccountMeta::new(u.a, false),
                AccountMeta::new(u.b, false),
                AccountMeta::new(u.lp, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data,
        }
    }

    fn add(&mut self, u: &User, max_a: u64, max_b: u64, min_lp: u64) -> TxResult {
        let ix = self.liquidity_ix(1, u, max_a, max_b, min_lp);
        self.send(ix, &u.kp)
    }

    fn remove(&mut self, u: &User, lp: u64, min_a: u64, min_b: u64) -> TxResult {
        let ix = self.liquidity_ix(2, u, lp, min_a, min_b);
        self.send(ix, &u.kp)
    }

    fn swap_ix(&self, u: &User, amount_in: u64, min_out: u64, a_to_b: bool) -> Instruction {
        let mut data = vec![3u8];
        data.extend_from_slice(&amount_in.to_le_bytes());
        data.extend_from_slice(&min_out.to_le_bytes());
        data.push(a_to_b as u8);
        let (src, dst) = if a_to_b { (u.a, u.b) } else { (u.b, u.a) };
        Instruction {
            program_id: PROGRAM_ID,
            accounts: vec![
                AccountMeta::new_readonly(u.kp.pubkey(), true),
                AccountMeta::new(self.pool, false),
                AccountMeta::new(self.vault_a, false),
                AccountMeta::new(self.vault_b, false),
                AccountMeta::new(src, false),
                AccountMeta::new(dst, false),
                AccountMeta::new_readonly(spl_token::ID, false),
            ],
            data,
        }
    }

    fn swap(&mut self, u: &User, amount_in: u64, min_out: u64, a_to_b: bool) -> TxResult {
        let ix = self.swap_ix(u, amount_in, min_out, a_to_b);
        self.send(ix, &u.kp)
    }

    fn reserves(&self) -> (u64, u64) {
        let d = self.svm.get_account(&self.pool).unwrap().data;
        (
            u64::from_le_bytes(d[168..176].try_into().unwrap()),
            u64::from_le_bytes(d[176..184].try_into().unwrap()),
        )
    }

    fn bal(&self, token_account: &Pubkey) -> u64 {
        get_spl_account::<spl_token::state::Account>(&self.svm, token_account).unwrap().amount
    }

    fn lp_supply(&self) -> u64 {
        get_spl_account::<spl_token::state::Mint>(&self.svm, &self.lp_mint).unwrap().supply
    }

    /// Pool seeded with `a`/`b` by a dedicated LP.
    fn seeded(a: u64, b: u64) -> (Self, User) {
        let mut env = Self::new();
        let lp = env.user(a, b);
        env.add(&lp, a, b, 0).unwrap();
        (env, lp)
    }
}

fn amm_err(res: TxResult, e: AmmError) {
    assert_ix_err(res, InstructionError::Custom(ERROR_BASE + e as u32));
}

fn assert_ix_err(res: TxResult, expected: InstructionError) {
    let err = res.expect_err("transaction should have failed").err;
    assert_eq!(err, TransactionError::InstructionError(0, expected));
}

// SPL Token error codes surfaced from failed CPIs.
const TOKEN_OWNER_MISMATCH: u32 = 4;
const TOKEN_MINT_MISMATCH: u32 = 3;

// ---------------------------------------------------------------- initialization

#[test]
fn initialize_creates_pool_vaults_and_lp_mint() {
    let env = Env::new();
    let pool = env.svm.get_account(&env.pool).unwrap();
    assert_eq!(pool.owner, PROGRAM_ID);
    assert_eq!(pool.data.len(), POOL_LEN);
    assert_eq!(pool.data[0], 1, "discriminator");
    assert_eq!(u16::from_le_bytes([pool.data[5], pool.data[6]]), FEE_BPS);

    for vault in [env.vault_a, env.vault_b] {
        let v = get_spl_account::<spl_token::state::Account>(&env.svm, &vault).unwrap();
        assert_eq!(v.owner, env.pool, "pool PDA is the vault authority");
    }
    let lp = get_spl_account::<spl_token::state::Mint>(&env.svm, &env.lp_mint).unwrap();
    assert_eq!(lp.mint_authority, Some(env.pool).into());
    assert!(lp.freeze_authority.is_none());
}

#[test]
fn initialize_rejects_unsorted_or_duplicate_mints() {
    let mut env = Env::bare();
    let (a, b) = (env.mint_a, env.mint_b);
    amm_err(env.send_payer(env.init_ix(b, a, FEE_BPS)), AmmError::MintsNotSorted);
    amm_err(env.send_payer(env.init_ix(a, a, FEE_BPS)), AmmError::MintsNotSorted);
}

#[test]
fn initialize_rejects_excessive_fee_and_double_init() {
    let mut env = Env::bare();
    let (a, b) = (env.mint_a, env.mint_b);
    amm_err(env.send_payer(env.init_ix(a, b, 1_001)), AmmError::FeeTooHigh);
    env.send_payer(env.init_ix(a, b, FEE_BPS)).unwrap();
    assert!(env.send_payer(env.init_ix(a, b, 100)).is_err(), "pool for a pair can only be created once");
}

// ---------------------------------------------------------------- liquidity

#[test]
fn first_deposit_locks_minimum_liquidity() {
    let mut env = Env::new();
    let alice = env.user(4_000 * ONE, 1_000 * ONE);
    env.add(&alice, 4_000 * ONE, 1_000 * ONE, 0).unwrap();
    let root = 2_000 * ONE; // sqrt(4000 * 1000) in base units
    assert_eq!(env.bal(&alice.lp), root - MINIMUM_LIQUIDITY);
    assert_eq!(env.reserves(), (4_000 * ONE, 1_000 * ONE));
}

#[test]
fn later_deposit_is_proportional_and_takes_only_what_is_needed() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 3_000 * ONE);
    let bob = env.user(100 * ONE, 1_000 * ONE);
    env.add(&bob, 100 * ONE, 1_000 * ONE, 0).unwrap();
    let taken_b = 1_000 * ONE - env.bal(&bob.b);
    assert_eq!(env.bal(&bob.a), 0);
    // Supply is sqrt(1000·3000) rounded down, so the ratio-exact amount is within 1 unit of 300.
    assert!(taken_b.abs_diff(300 * ONE) <= 1, "only the 1:3 ratio amount of B is taken, got {taken_b}");
    assert_eq!(env.reserves(), (1_100 * ONE, 3_000 * ONE + taken_b));

    // Withdrawing straight away returns no more than was deposited.
    let lp = env.bal(&bob.lp);
    env.remove(&bob, lp, 0, 0).unwrap();
    assert!(env.bal(&bob.a) <= 100 * ONE && env.bal(&bob.b) <= 1_000 * ONE);
}

#[test]
fn removing_everything_leaves_locked_liquidity_behind() {
    let (mut env, alice) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let lp = env.bal(&alice.lp);
    env.remove(&alice, lp, 0, 0).unwrap();
    let (ra, rb) = env.reserves();
    assert!(ra > 0 && rb > 0, "locked MINIMUM_LIQUIDITY keeps the pool alive");
    assert_eq!(env.lp_supply(), 0);
    assert!(env.bal(&alice.a) >= 1_000 * ONE - 1_000);
}

#[test]
fn fees_accrue_to_liquidity_providers() {
    let (mut env, alice) = Env::seeded(1_000_000 * ONE, 1_000_000 * ONE);
    let trader = env.user(10_000_000 * ONE, 10_000_000 * ONE);
    for i in 0..20 {
        env.swap(&trader, 50_000 * ONE, 0, i % 2 == 0).unwrap();
    }
    let lp = env.bal(&alice.lp);
    env.remove(&alice, lp, 0, 0).unwrap();
    let (got_a, got_b) = (env.bal(&alice.a) as u128, env.bal(&alice.b) as u128);
    let put = 1_000_000u128 * ONE as u128;
    assert!(got_a * got_b > put * put, "LP position grew from swap fees");
}

#[test]
fn deposit_and_withdraw_slippage_bounds() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let bob = env.user(10 * ONE, 10 * ONE);
    amm_err(env.add(&bob, 10 * ONE, 10 * ONE, 10 * ONE + 1), AmmError::SlippageExceeded);
    env.add(&bob, 10 * ONE, 10 * ONE, 10 * ONE).unwrap();
    let lp = env.bal(&bob.lp);
    amm_err(env.remove(&bob, lp, 10 * ONE + 1, 0), AmmError::SlippageExceeded);
}

// ---------------------------------------------------------------- swaps

#[test]
fn swap_follows_constant_product_with_fee() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 2_000 * ONE);
    let t = env.user(10 * ONE, 0);
    let expected = swap_out(10 * ONE, 1_000 * ONE, 2_000 * ONE, FEE_BPS).unwrap();
    let meta = env.swap(&t, 10 * ONE, expected, true).unwrap();
    println!("swap CU: {}", meta.compute_units_consumed);
    assert_eq!(env.bal(&t.b), expected);
    assert_eq!(env.reserves(), (1_010 * ONE, 2_000 * ONE - expected));
}

#[test]
fn round_trip_swap_loses_to_fees() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let t = env.user(100 * ONE, 0);
    env.swap(&t, 100 * ONE, 0, true).unwrap();
    let b = env.bal(&t.b);
    env.swap(&t, b, 0, false).unwrap();
    assert!(env.bal(&t.a) < 100 * ONE);
}

#[test]
fn swap_slippage_bound() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let t = env.user(10 * ONE, 0);
    let out = swap_out(10 * ONE, 1_000 * ONE, 1_000 * ONE, FEE_BPS).unwrap();
    amm_err(env.swap(&t, 10 * ONE, out + 1, true), AmmError::SlippageExceeded);
}

#[test]
fn k_never_decreases_over_random_swaps() {
    let (mut env, _) = Env::seeded(50_000 * ONE, 80_000 * ONE);
    let t = env.user(1_000_000 * ONE, 1_000_000 * ONE);
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let (mut ra, mut rb) = env.reserves();
    for _ in 0..120 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let a_to_b = seed & 1 == 0;
        let amount = 1 + seed % (5_000 * ONE);
        if env.swap(&t, amount, 0, a_to_b).is_err() {
            continue;
        }
        let (na, nb) = env.reserves();
        assert!(na as u128 * nb as u128 >= ra as u128 * rb as u128, "k decreased");
        assert!(env.bal(&env.vault_a) >= na && env.bal(&env.vault_b) >= nb, "vaults cover reserves");
        (ra, rb) = (na, nb);
    }
}

// ---------------------------------------------------------------- attacks

#[test]
fn donation_does_not_move_the_price() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let whale = env.user(1_000_000 * ONE, 0);
    let (ma, va) = (env.mint_a, env.vault_a);
    Transfer::new(&mut env.svm, &whale.kp, &ma, &va, 1_000_000 * ONE).source(&whale.a).send().unwrap();
    assert_eq!(env.reserves(), (1_000 * ONE, 1_000 * ONE), "reserves ignore the donation");

    let t = env.user(10 * ONE, 0);
    let expected = swap_out(10 * ONE, 1_000 * ONE, 1_000 * ONE, FEE_BPS).unwrap();
    env.swap(&t, 10 * ONE, 0, true).unwrap();
    assert_eq!(env.bal(&t.b), expected);
}

#[test]
fn fake_token_program_is_rejected() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let t = env.user(10 * ONE, 0);
    let mut ix = env.swap_ix(&t, 10 * ONE, 0, true);
    ix.accounts[6] = AccountMeta::new_readonly(SYSTEM, false);
    assert_ix_err(env.send(ix, &t.kp), InstructionError::IncorrectProgramId);
}

/// Drain attempt: pass your own token account as the *input* vault, so you "pay" yourself
/// while the real output vault pays you. Rejected because vaults are pinned in pool state.
#[test]
fn substituted_input_vault_is_rejected() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let mallory = env.user(100 * ONE, 0);
    let (ma, mk) = (env.mint_a, mallory.kp.insecure_clone());
    let own_a = CreateAccount::new(&mut env.svm, &mk, &ma).owner(&mk.pubkey()).send().unwrap();
    let mut ix = env.swap_ix(&mallory, 100 * ONE, 0, true);
    ix.accounts[2] = AccountMeta::new(own_a, false); // own account as vault_a
    amm_err(env.send(ix, &mallory.kp), AmmError::AccountMismatch);
    assert_eq!(env.bal(&env.vault_b), 1_000 * ONE, "output vault untouched");
}

/// A forged pool account (same layout, inflated reserves) owned by another program.
#[test]
fn forged_pool_account_is_rejected() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let mut data = env.svm.get_account(&env.pool).unwrap().data;
    data[176..184].copy_from_slice(&(1_000_000_000 * ONE).to_le_bytes());
    let fake = Pubkey::new_unique();
    env.svm
        .set_account(
            fake,
            Account {
                lamports: 10_000_000,
                data,
                owner: Pubkey::new_unique(),
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();
    let t = env.user(10 * ONE, 0);
    let mut ix = env.swap_ix(&t, 10 * ONE, 0, true);
    ix.accounts[1] = AccountMeta::new(fake, false);
    assert_ix_err(env.send(ix, &t.kp), InstructionError::InvalidAccountOwner);
}

#[test]
fn user_must_sign() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let victim = env.user(10 * ONE, 0);
    let mallory = env.user(0, 0);
    let mut ix = env.swap_ix(&victim, 10 * ONE, 0, true);
    ix.accounts[0] = AccountMeta::new_readonly(victim.kp.pubkey(), false);
    ix.accounts[5] = AccountMeta::new(mallory.b, false);
    assert_ix_err(env.send(ix, &mallory.kp), InstructionError::MissingRequiredSignature);
}

#[test]
fn cannot_burn_someone_elses_lp() {
    let (mut env, alice) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let mallory = env.user(0, 0);
    let before = env.bal(&alice.lp);
    let thief = User { kp: mallory.kp.insecure_clone(), a: mallory.a, b: mallory.b, lp: alice.lp };
    assert_ix_err(env.remove(&thief, 1_000, 0, 0), InstructionError::Custom(TOKEN_OWNER_MISMATCH));
    assert_eq!(env.bal(&alice.lp), before);
}

#[test]
fn wrong_mint_destination_fails_in_token_program() {
    let (mut env, _) = Env::seeded(1_000 * ONE, 1_000 * ONE);
    let t = env.user(10 * ONE, 0);
    let mut ix = env.swap_ix(&t, 10 * ONE, 0, true);
    ix.accounts[5] = AccountMeta::new(t.a, false); // receive B into an A account
    assert_ix_err(env.send(ix, &t.kp), InstructionError::Custom(TOKEN_MINT_MISMATCH));
}
