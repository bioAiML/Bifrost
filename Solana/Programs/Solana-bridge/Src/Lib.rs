use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use liboqs_rs::dilithium3;
use rand::rngs::OsRng;
use anchor_lang::solana_program::{hash::hash, compute_budget};
use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize)]
pub struct TokensLockedEvent {
    pub user: Pubkey,
    pub amount: u64,
    pub nonce: u64,
    pub proof: [u8; 32],
    pub block_height: u64,
}

declare_id!("ReplaceWithActualDeployedID");

#[program]
pub mod solana_bridge {
    use super::*;

    #[error_code]
    pub enum BridgeError {
        #[msg("Invalid amount")] InvalidAmount,
        #[msg("Insufficient balance")] InsufficientBalance,
        #[msg("Verification failed")] VerificationFailed,
        #[msg("Bridge paused")] Paused,
        #[msg("Rate limit exceeded")] RateLimitExceeded,
        #[msg("Replay detected")] ReplayDetected,
        #[msg("Timelock not expired")] TimelockNotExpired,
        #[msg("Deadline not expired")] DeadlineNotExpired,
    }

    #[account]
    pub struct BridgeConfig {
        pub admin: Pubkey,
        pub paused: bool,
        pub max_transfer_amount: u64,
        pub total_locked: u64,
        pub timelock: i64,
        pub validators: Vec<Pubkey>,
    }

    #[account]
    pub struct ProcessedProofs {
        pub proofs: Vec<[u8; 32]>,
    }

    #[account]
    pub struct PendingTransfer {
        pub user: Pubkey,
        pub amount: u64,
        pub nonce: u64,
        pub deadline: i64,
    }

    #[event]
    pub struct TokensLocked {
        pub user: Pubkey,
        pub amount: u64,
        pub nonce: u64,
        pub proof: [u8; 32],
        pub block_height: u64,
    }

    pub fn initialize(ctx: Context<Initialize>, max_transfer_amount: u64, validators: Vec<Pubkey>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        config.admin = ctx.accounts.admin.key();
        config.paused = false;
        config.max_transfer_amount = max_transfer_amount;
        config.total_locked = 0;
        config.timelock = 0;
        config.validators = validators;
        require!(validators.len() >= 5, BridgeError::VerificationFailed);
        Ok(())
    }

    pub fn lock_tokens(ctx: Context<LockTokens>, amount: u64) -> Result<()> {
        compute_budget::set_compute_unit_limit(200_000)?;
        let config = &ctx.accounts.config;
        if config.paused { return err!(BridgeError::Paused); }
        if amount == 0 { return err!(BridgeError::InvalidAmount); }
        if amount > config.max_transfer_amount { return err!(BridgeError::RateLimitExceeded); }

        let nonce = OsRng.next_u64();
        let proof = hash(&[&ctx.accounts.user.key().to_bytes(), &amount.to_le_bytes(), &nonce.to_le_bytes()]).to_bytes();
        let processed = &mut ctx.accounts.processed_proofs;
        if processed.proofs.contains(&proof) { return err!(BridgeError::ReplayDetected); }
        processed.proofs.push(proof);

        let pending = &mut ctx.accounts.pending_transfer;
        pending.user = ctx.accounts.user.key();
        pending.amount = amount;
        pending.nonce = nonce;
        pending.deadline = Clock::get()?.unix_timestamp + 300;

        let new_total = config.total_locked.checked_add(amount).ok_or(BridgeError::InvalidAmount)?;
        ctx.accounts.config.total_locked = new_total;

        let cpi_accounts = Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.bridge_token_account.to_account_info(),
            authority: ctx.accounts.user.to_account_info(),
        };
        let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
        token::transfer(cpi_ctx, amount)?;

        emit!(TokensLocked {
            user: ctx.accounts.user.key(),
            amount,
            nonce,
            proof,
            block_height: Clock::get()?.slot,
        });
        Ok(())
    }

    pub fn unlock_tokens(ctx: Context<UnlockTokens>, amount: u64, nonce: u64, proof: [u8; 32], signatures: Vec<[u8; 64]>, block_height: u64) -> Result<()> {
        compute_budget::set_compute_unit_limit(300_000)?;
        let config = &ctx.accounts.config;
        if config.paused { return err!(BridgeError::Paused); }
        if amount == 0 { return err!(BridgeError::InvalidAmount); }
        if block_height > Clock::get()?.slot + 2 { return err!(BridgeError::VerificationFailed); }

        let message = hash(&[&ctx.accounts.user.key().to_bytes(), &amount.to_le_bytes(), &nonce.to_le_bytes()]).to_bytes();
        let mut valid_signatures = 0;
        for sig in signatures {
            for validator in &config.validators {
                if verify_dilithium(&sig, &message, validator.as_ref()) {
                    valid_signatures += 1;
                    break;
                }
            }
        }
        if valid_signatures < 3 { return err!(BridgeError::VerificationFailed); }
        if proof != message { return err!(BridgeError::VerificationFailed); }

        let processed = &mut ctx.accounts.processed_proofs;
        if processed.proofs.contains(&proof) { return err!(BridgeError::ReplayDetected); }
        processed.proofs.push(proof);

        let new_total = config.total_locked.checked_sub(amount).ok_or(BridgeError::InvalidAmount)?;
        ctx.accounts.config.total_locked = new_total;

        let seeds = &[b"bridge".as_ref(), &[ctx.bumps.bridge_token_account]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.bridge_token_account.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.bridge_token_account.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;

        Ok(())
    }

    pub fn revert_lock(ctx: Context<RevertLock>) -> Result<()> {
        let now = Clock::get()?.unix_timestamp;
        let pending = &ctx.accounts.pending_transfer;
        if now < pending.deadline { return err!(BridgeError::DeadlineNotExpired); }

        let seeds = &[b"bridge".as_ref(), &[ctx.bumps.bridge_token_account]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.bridge_token_account.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.bridge_token_account.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer);
        token::transfer(cpi_ctx, pending.amount)?;

        ctx.accounts.config.total_locked = ctx.accounts.config.total_locked.checked_sub(pending.amount).ok_or(BridgeError::InvalidAmount)?;
        Ok(())
    }

    pub fn initiate_recovery(ctx: Context<Recovery>, amount: u64, to: Pubkey) -> Result<()> {
        let config = &mut ctx.accounts.config;
        require_keys_eq!(ctx.accounts.admin.key(), config.admin, BridgeError::VerificationFailed);
        if config.timelock == 0 {
            config.timelock = Clock::get()?.unix_timestamp + 24 * 3600;
            return Ok(());
        }
        if Clock::get()?.unix_timestamp < config.timelock { return err!(BridgeError::TimelockNotExpired); }

        let seeds = &[b"bridge".as_ref(), &[ctx.bumps.bridge_token_account]];
        let signer = &[&seeds[..]];
        let cpi_accounts = Transfer {
            from: ctx.accounts.bridge_token_account.to_account_info(),
            to: ctx.accounts.recovery_account.to_account_info(),
            authority: ctx.accounts.bridge_token_account.to_account_info(),
        };
        let cpi_ctx = CpiContext::new_with_signer(ctx.accounts.token_program.to_account_info(), cpi_accounts, signer);
        token::transfer(cpi_ctx, amount)?;

        config.timelock = 0;
        Ok(())
    }

    pub fn update_validators(ctx: Context<UpdateValidators>, new_validators: Vec<Pubkey>) -> Result<()> {
        require_keys_eq!(ctx.accounts.admin.key(), ctx.accounts.config.admin, BridgeError::VerificationFailed);
        let config = &mut ctx.accounts.config;
        require!(new_validators.len() >= 5, BridgeError::VerificationFailed);
        config.validators = new_validators;
        Ok(())
    }

    pub fn pause_bridge(ctx: Context<PauseBridge>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        require_keys_eq!(ctx.accounts.admin.key(), config.admin, BridgeError::VerificationFailed);
        config.paused = true;
        Ok(())
    }

    pub fn unpause_bridge(ctx: Context<PauseBridge>) -> Result<()> {
        let config = &mut ctx.accounts.config;
        require_keys_eq!(ctx.accounts.admin.key(), config.admin, BridgeError::VerificationFailed);
        config.paused = false;
        Ok(())
    }

    pub fn get_total_locked(ctx: Context<GetConfig>) -> Result<u64> {
        Ok(ctx.accounts.config.total_locked)
    }

    pub fn get_balance(ctx: Context<GetBalance>) -> Result<u64> {
        Ok(ctx.accounts.user_token_account.amount)
    }

    fn verify_dilithium(sig: &[u8; 64], message: &[u8; 32], pubkey: &[u8]) -> bool {
        if let Ok(pk) = dilithium3::PublicKey::from_bytes(pubkey) {
            if let Ok(signature) = dilithium3::Signature::from_bytes(sig) {
                return pk.verify(message, &signature).is_ok();
            }
        }
        false
    }
}

#[derive(Accounts)]
pub struct Initialize<'info> {
    #[account(init, payer = admin, space = 8 + 32 + 1 + 8 + 8 + 8 + 5 * 32)] pub config: Account<'info, BridgeConfig>,
    #[account(init, payer = admin, space = 8 + 32 * 100)] pub processed_proofs: Account<'info, ProcessedProofs>,
    #[account(mut)] pub admin: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct LockTokens<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    #[account(mut)] pub processed_proofs: Account<'info, ProcessedProofs>,
    #[account(init, payer = user, space = 8 + 32 + 8 + 8 + 8)] pub pending_transfer: Account<'info, PendingTransfer>,
    #[account(mut)] pub user: Signer<'info>,
    #[account(mut)] pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut, seeds = [b"bridge"], bump)] pub bridge_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct UnlockTokens<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    #[account(mut)] pub processed_proofs: Account<'info, ProcessedProofs>,
    #[account(mut)] pub user: Signer<'info>,
    #[account(mut)] pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut, seeds = [b"bridge"], bump)] pub bridge_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct RevertLock<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    #[account(mut, close = user)] pub pending_transfer: Account<'info, PendingTransfer>,
    #[account(mut)] pub user: Signer<'info>,
    #[account(mut)] pub user_token_account: Account<'info, TokenAccount>,
    #[account(mut, seeds = [b"bridge"], bump)] pub bridge_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct Recovery<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    #[account(mut)] pub admin: Signer<'info>,
    #[account(mut)] pub recovery_account: Account<'info, TokenAccount>,
    #[account(mut, seeds = [b"bridge"], bump)] pub bridge_token_account: Account<'info, TokenAccount>,
    pub token_program: Program<'info, Token>,
}

#[derive(Accounts)]
pub struct UpdateValidators<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct PauseBridge<'info> {
    #[account(mut)] pub config: Account<'info, BridgeConfig>,
    pub admin: Signer<'info>,
}

#[derive(Accounts)]
pub struct GetConfig<'info> {
    pub config: Account<'info, BridgeConfig>,
}

#[derive(Accounts)]
pub struct GetBalance<'info> {
    pub user_token_account: Account<'info, TokenAccount>,
}
