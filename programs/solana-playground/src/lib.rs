// src/lib.rs
use anchor_lang::prelude::*;
use anchor_lang::system_program;
use anchor_lang::solana_program::program::invoke;
use anchor_lang::solana_program::system_instruction;
use anchor_lang::solana_program::pubkey;

declare_id!("Fu5sXvLemQ5meB4y3GWM4oacD2uDwbF8URFh2WpmCMeR");

// --- Constants ---
const ROOM_SEED_PREFIX: &[u8] = b"room";
const VAULT_SEED_PREFIX: &[u8] = b"vault";

// --- Fee Constants ---
const CREATOR_FEE_BASIS_POINTS: u64 = 500; // 5%
const SERVICE_FEE_BASIS_POINTS: u64 = 300; // 3%
const SERVICE_WALLET_PUBKEY: Pubkey = pubkey!("FDKFLU6mUjfYZRRSrqbS9CPH87MFpae8JSH9Ddt79oRN"); // !!! REPLACE THIS !!!
const BASIS_POINTS_DENOMINATOR: u64 = 10000;

// --- Data Size Constants (FOR MANUAL CALCULATION) ---
const MAX_ROOM_SEED_LEN: usize = 32;
const MAX_PLAYERS_ALLOWED: usize = 100; // Max players for Vec allocation


#[program]
pub mod palapa_fun_rooms {
    use super::*;

    /// Creates a new game room associated with the creator.
    pub fn create_room(
        ctx: Context<CreateRoom>,
        room_seed: String,
        max_players: u16,
        entry_fee: u64,
    ) -> Result<()> {
        // Input validation using constants
        require!(max_players > 1, PalapaError::InvalidMaxPlayers);
        require!(!room_seed.is_empty() && room_seed.len() <= MAX_ROOM_SEED_LEN, PalapaError::InvalidRoomSeed);
        // Check against the hardcoded allocation limit used in calculate_space
        require!(max_players as usize <= MAX_PLAYERS_ALLOWED, PalapaError::MaxPlayersExceedsLimit);

        let room_data = &mut ctx.accounts.room_data;
        let clock = Clock::get()?;

        // Initialize room data
        room_data.creator = *ctx.accounts.creator.key;
        room_data.room_seed = room_seed;
        room_data.bump = ctx.bumps.room_data; // Access bumps from context
        room_data.vault_bump = ctx.bumps.room_vault; // Access bumps from context
        room_data.status = RoomStatus::OpenForJoining;
        room_data.winner = None;
        room_data.max_players = max_players; // Store the actual limit for this room
        room_data.entry_fee = entry_fee;
        room_data.players = Vec::with_capacity(max_players as usize);
        room_data.creation_timestamp = clock.unix_timestamp;
        room_data.end_timestamp = None;

        msg!("Room created by {} with seed '{}'", room_data.creator, room_data.room_seed);
        msg!("Max players: {}, Entry fee: {} lamports", room_data.max_players, room_data.entry_fee);
        Ok(())
    }

    /// Allows a player to join an existing, open room by paying the entry fee.
    pub fn join_room(ctx: Context<JoinRoom>, _room_seed: String) -> Result<()> {
        let room_data = &mut ctx.accounts.room_data;
        let player = &ctx.accounts.player;
        let vault = &ctx.accounts.room_vault;
        let system_program_account = &ctx.accounts.system_program;

        require!(room_data.status == RoomStatus::OpenForJoining, PalapaError::RoomNotJoinable);
        require!(room_data.players.len() < room_data.max_players as usize, PalapaError::RoomFull);
        require!(!room_data.players.contains(player.key), PalapaError::PlayerAlreadyJoined);

        if room_data.entry_fee > 0 {
            let transfer_instruction = system_instruction::transfer(
                player.key,
                vault.key,
                room_data.entry_fee,
            );
            invoke(
                &transfer_instruction,
                &[
                    player.to_account_info(),
                    vault.to_account_info(),
                    system_program_account.to_account_info(),
                ],
            )?;
             msg!("Player {} paid {} lamports entry fee", player.key(), room_data.entry_fee);
        } else {
             msg!("Player {} joined a free room", player.key());
        }

        room_data.players.push(*player.key);
        msg!("Player {} joined the room. Total players: {}", player.key(), room_data.players.len());

        if room_data.players.len() == room_data.max_players as usize {
            room_data.status = RoomStatus::InProgress;
            msg!("Room is now full and in progress.");
        }
        Ok(())
    }

    /// Called by the room creator to declare the winner and distribute the vault funds.
    pub fn announce_winner(ctx: Context<AnnounceWinner>, _room_seed: String, winner_pubkey: Pubkey) -> Result<()> {
        let room_data = &mut ctx.accounts.room_data;
        let vault = &ctx.accounts.room_vault;
        let winner_account = &ctx.accounts.winner_account;
        let creator_account = &ctx.accounts.creator;
        let service_fee_recipient = &ctx.accounts.service_fee_recipient;
        let system_program_account = &ctx.accounts.system_program;
        let clock = Clock::get()?;

        require!(room_data.status == RoomStatus::InProgress, PalapaError::RoomNotInProgress);
        require!(room_data.players.contains(&winner_pubkey), PalapaError::WinnerNotInRoom);

        room_data.winner = Some(winner_pubkey);
        room_data.status = RoomStatus::Finished;
        room_data.end_timestamp = Some(clock.unix_timestamp);
        msg!("Winner announced: {}", winner_pubkey);

        let vault_rent = Rent::get()?.minimum_balance(0);
        let total_prize_amount = vault.lamports().checked_sub(vault_rent).unwrap_or(0);

        msg!("Vault Balance: {}, Vault Rent: {}", vault.lamports(), vault_rent);
        msg!("Total prize pool (excluding rent): {} lamports", total_prize_amount);

        let creator_key_bytes = room_data.creator.key().to_bytes();
        let room_seed_bytes = room_data.room_seed.as_bytes();
        let vault_bump_slice = &[ctx.bumps.room_vault];
        let signer_seeds: &[&[&[u8]]] = &[&[
            VAULT_SEED_PREFIX, creator_key_bytes.as_ref(), room_seed_bytes, vault_bump_slice,
        ]];

        if total_prize_amount > 0 {
            let creator_fee = total_prize_amount.checked_mul(CREATOR_FEE_BASIS_POINTS).ok_or(PalapaError::CalculationOverflow)?.checked_div(BASIS_POINTS_DENOMINATOR).ok_or(PalapaError::CalculationOverflow)?;
            let service_fee = total_prize_amount.checked_mul(SERVICE_FEE_BASIS_POINTS).ok_or(PalapaError::CalculationOverflow)?.checked_div(BASIS_POINTS_DENOMINATOR).ok_or(PalapaError::CalculationOverflow)?;
            let fees_total = creator_fee.checked_add(service_fee).ok_or(PalapaError::CalculationOverflow)?;
            let winner_share_prize = total_prize_amount.checked_sub(fees_total).ok_or(PalapaError::CalculationOverflow)?;
            let winner_total_receive = winner_share_prize.checked_add(vault_rent).ok_or(PalapaError::CalculationOverflow)?;

            msg!("Calculated Creator Fee: {}", creator_fee);
            msg!("Calculated Service Fee: {}", service_fee);
            msg!("Calculated Winner Share (Prize): {}", winner_share_prize);
            msg!("Total to Winner (Share + Rent): {}", winner_total_receive);
            // Removed redundant check: require!(winner_share_prize >= 0, ...);

            if creator_fee > 0 {
                system_program::transfer(CpiContext::new_with_signer(system_program_account.to_account_info(), system_program::Transfer { from: vault.to_account_info(), to: creator_account.to_account_info() }, signer_seeds), creator_fee)?;
                msg!("Transferred creator fee {} to {}", creator_fee, creator_account.key());
            }
            if service_fee > 0 {
                system_program::transfer(CpiContext::new_with_signer(system_program_account.to_account_info()   , system_program::Transfer { from: vault.to_account_info(), to: service_fee_recipient.to_account_info() }, signer_seeds), service_fee)?;
                msg!("Transferred service fee {} to {}", service_fee, service_fee_recipient.key());
            }
            if winner_total_receive > 0 {
                system_program::transfer(CpiContext::new_with_signer(system_program_account.to_account_info(), system_program::Transfer { from: vault.to_account_info(), to: winner_account.to_account_info() }, signer_seeds), winner_total_receive)?;
                msg!("Transferred total winner amount {} to {}", winner_total_receive, winner_account.key());
            }

        } else {
             msg!("No prize pool to distribute fees from.");
             let current_vault_balance = vault.lamports();
             if current_vault_balance > 0 {
                 msg!("Transferring remaining vault balance (rent: {}) to winner", current_vault_balance);
                 system_program::transfer(CpiContext::new_with_signer(system_program_account.to_account_info(), system_program::Transfer { from: vault.to_account_info(), to: winner_account.to_account_info() }, signer_seeds), current_vault_balance)?;
                 msg!("Transferred remaining vault balance {} to winner {}", current_vault_balance, winner_account.key());
             } else {
                 msg!("Vault was already empty.");
             }
        }

        let vault_lamports_after = vault.to_account_info().lamports();
        require!(vault_lamports_after == 0, PalapaError::VaultNotEmptyAfterPayout);
        msg!("Vault is now empty.");

        Ok(())
    }

     /// Allows the creator to cancel a room IF it's OpenForJoining/Created AND has no players.
     pub fn cancel_room(ctx: Context<CancelRoom>, _room_seed: String) -> Result<()> {
        let room_data = &mut ctx.accounts.room_data;
        let vault = &ctx.accounts.room_vault;
        let creator = &ctx.accounts.creator;
        let system_program_account = &ctx.accounts.system_program;
        let clock = Clock::get()?;

        require!(room_data.status == RoomStatus::OpenForJoining || room_data.status == RoomStatus::Created, PalapaError::CannotCancelRoomState);
        require!(room_data.players.is_empty(), PalapaError::CannotCancelRoomPlayersJoined);

        room_data.status = RoomStatus::Cancelled;
        room_data.end_timestamp = Some(clock.unix_timestamp);
        msg!("Room cancelled by creator {}", creator.key());

        let vault_balance = vault.lamports();
        if vault_balance > 0 {
             let creator_key_bytes = room_data.creator.key().to_bytes();
             let room_seed_bytes = room_data.room_seed.as_bytes();
             let vault_bump_slice = &[ctx.bumps.room_vault];
             let signer_seeds: &[&[&[u8]]] = &[&[
                 VAULT_SEED_PREFIX, creator_key_bytes.as_ref(), room_seed_bytes, vault_bump_slice,
             ]];

            msg!("Attempting to recover {} lamports (rent) from vault to creator", vault_balance);
            system_program::transfer(CpiContext::new_with_signer(system_program_account.to_account_info(), system_program::Transfer { from: vault.to_account_info(), to: creator.to_account_info() }, signer_seeds), vault_balance)?;
            msg!("Successfully recovered {} lamports from vault to creator.", vault_balance);

            let vault_lamports_after = vault.to_account_info().lamports();
            require!(vault_lamports_after == 0, PalapaError::VaultNotEmptyAfterPayout);
        } else {
             msg!("Vault was already empty, no rent to recover.");
        }
        Ok(())
    }
}

// --- Account Structs & Contexts ---

#[derive(Accounts)]
#[instruction(room_seed: String, max_players: u16)]
pub struct CreateRoom<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        init,
        payer = creator,
        space = RoomData::calculate_space(max_players, &room_seed),
        seeds = [ROOM_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump
    )]
    pub room_data: Account<'info, RoomData>,
    /// CHECK: This PDA is initialized here using specific seeds and payer.
    /// Space is 0, owner is SystemProgram. Anchor handles the init checks.
    /// It only holds lamports transferred via CPI, used solely as a vault.
    #[account(
        init,
        payer = creator,
        space = 0,
        seeds = [VAULT_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump,
        owner = system_program::ID
    )]
    pub room_vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(room_seed: String)]
pub struct JoinRoom<'info> {
    #[account(mut)]
    pub player: Signer<'info>,
    #[account(
        mut,
        seeds = [ROOM_SEED_PREFIX, room_data.creator.as_ref(), room_seed.as_bytes()],
        bump = room_data.bump,
    )]
    pub room_data: Account<'info, RoomData>,
    /// CHECK: Vault PDA corresponding to the room. Mutable for receiving entry fees via CPI. Seeds verified by Anchor.
    #[account(
        mut,
        seeds = [VAULT_SEED_PREFIX, room_data.creator.as_ref(), room_seed.as_bytes()],
        bump = room_data.vault_bump
    )]
    pub room_vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(room_seed: String, winner_pubkey: Pubkey)]
pub struct AnnounceWinner<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        mut,
        seeds = [ROOM_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump = room_data.bump, // Use stored bump
        has_one = creator @ PalapaError::Unauthorized
    )]
    pub room_data: Account<'info, RoomData>,
    /// CHECK: Vault PDA corresponding to the room. Mutable for transferring funds out via CPI signed by PDA seeds. Seeds verified by Anchor. Bump derived for transfer.
    #[account(
        mut,
        seeds = [VAULT_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump
    )]
    pub room_vault: AccountInfo<'info>,
    /// CHECK: Winner account, mutable for receiving funds. Checked by constraint.
    #[account(
        mut,
        constraint = winner_account.key() == winner_pubkey @ PalapaError::WinnerAccountMismatch
    )]
    pub winner_account: AccountInfo<'info>,
    /// CHECK: Service fee account, mutable for receiving funds. Checked by constraint.
    #[account(
        mut,
        constraint = service_fee_recipient.key() == SERVICE_WALLET_PUBKEY @ PalapaError::InvalidServiceWallet
    )]
    pub service_fee_recipient: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}


#[derive(Accounts)]
#[instruction(room_seed: String)]
pub struct CancelRoom<'info> {
    #[account(mut)]
    pub creator: Signer<'info>,
    #[account(
        mut,
        seeds = [ROOM_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump = room_data.bump, // Use stored bump
        has_one = creator @ PalapaError::Unauthorized,
        // close = creator // Optional: Close room data account
    )]
    pub room_data: Account<'info, RoomData>,
    /// CHECK: Vault PDA corresponding to the room. Mutable for transferring rent back via CPI signed by PDA seeds. Seeds verified by Anchor. Bump derived for transfer.
    #[account(
        mut,
        seeds = [VAULT_SEED_PREFIX, creator.key().as_ref(), room_seed.as_bytes()],
        bump
        // close = creator // Optional: Close vault account (AFTER transfer)
    )]
    pub room_vault: AccountInfo<'info>,
    pub system_program: Program<'info, System>,
}

// --- Account Data Structures & Enums/Errors ---

#[account]
pub struct RoomData {
    pub creator: Pubkey,
    pub room_seed: String,
    pub bump: u8,
    pub vault_bump: u8,
    pub status: RoomStatus,
    pub winner: Option<Pubkey>,
    pub max_players: u16,
    pub entry_fee: u64,
    pub players: Vec<Pubkey>,
    pub creation_timestamp: i64,
    pub end_timestamp: Option<i64>,
}

impl RoomData {
    // Function to calculate space manually
    // Use _max_players_request to silence warning if not directly used in calculation
    pub fn calculate_space(_max_players_request: u16, room_seed: &str) -> usize {
        let players_capacity_for_space = MAX_PLAYERS_ALLOWED;

        8 + // Anchor discriminator
        32 + // creator: Pubkey
        (4 + room_seed.len()) + // room_seed: String (variable length)
        // Or fixed: (4 + MAX_ROOM_SEED_LEN) +
        1 + // bump: u8
        1 + // vault_bump: u8
        RoomStatus::SPACE + // status: RoomStatus
        (1 + 32) + // winner: Option<Pubkey>
        2 + // max_players: u16
        8 + // entry_fee: u64
        (4 + players_capacity_for_space * 32) + // players: Vec<Pubkey>
        8 + // creation_timestamp: i64
        (1 + 8) + // end_timestamp: Option<i64>
        100 // Buffer
    }
}


#[derive(AnchorSerialize, AnchorDeserialize, Clone, PartialEq, Eq, Debug)]
pub enum RoomStatus {
    Created,
    OpenForJoining,
    InProgress,
    Finished,
    Cancelled,
}

impl RoomStatus {
    const SPACE: usize = 1; // Size of the discriminant
}


#[error_code]
pub enum PalapaError {
    #[msg("Invalid number of maximum players specified (must be > 1).")] InvalidMaxPlayers, // 6000
    #[msg("Entry fee must be greater than zero.")] InvalidEntryFee, // 6001
    #[msg("Room seed is invalid (empty or too long).")] InvalidRoomSeed, // 6002
    #[msg("Could not find PDA bump seed.")] BumpSeedNotFound, // 6003
    #[msg("Room is not in the correct state to be joined.")] RoomNotJoinable, // 6004
    #[msg("The room is already full.")] RoomFull, // 6005
    #[msg("This player has already joined the room.")] PlayerAlreadyJoined, // 6006
    #[msg("The room is not in progress, cannot announce winner.")] RoomNotInProgress, // 6007
    #[msg("The declared winner is not listed as a player in this room.")] WinnerNotInRoom, // 6008
    #[msg("The provided winner account does not match the winner pubkey.")] WinnerAccountMismatch, // 6009
    #[msg("Vault account was not empty after payout/recovery. Check transfer logic.")] VaultNotEmptyAfterPayout, // 6010
    #[msg("Unauthorized: Only the room creator can perform this action.")] Unauthorized, // 6011
    #[msg("Room cannot be cancelled in its current state (must be Open/Created).")] CannotCancelRoomState, // 6012
    #[msg("Room cannot be cancelled because players have already joined.")] CannotCancelRoomPlayersJoined, // 6013
    #[msg("Arithmetic overflow during fee or payout calculation.")] CalculationOverflow, // 6014
    #[msg("The provided service fee recipient account does not match the expected address.")] InvalidServiceWallet, // 6015
    #[msg("Insufficient funds in vault to cover calculated fees and payout (negative prize share).")] InsufficientFundsForPayout, // 6016
    #[msg("Requested max players exceeds the program's limit used for space allocation.")] MaxPlayersExceedsLimit, // 6017
    #[msg("Invalid Creator account provided for seed derivation.")] InvalidCreator, // 6018
}