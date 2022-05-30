use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint::ProgramResult,
    msg,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::{IsInitialized, Pack},
    pubkey::Pubkey,
    sysvar::{rent::Rent, Sysvar},
};

use spl_token::state::Account as TokenAccount;

use crate::{error::EscrowError, instruction::EscrowInstruction, state::Escrow};

pub struct Processor;
impl Processor {
    pub fn process(
        program_id: &Pubkey, 
        accounts: &[AccountInfo], 
        instruction_data: &[u8]
    ) -> ProgramResult {
        let instruction = EscrowInstruction::unpack(instruction_data)?;

        match instruction {
            EscrowInstruction::InitEscrow { amount } => {
                msg!("Instruction: InitEscrow");
                Self::process_init_escrow(accounts, amount, program_id)
            }
            EscrowInstruction::Withdraw { amount } => {
                msg!("Instruction: Withdraw");
                Self::process_withdraw(accounts, amount, program_id)
            }
        }
    }

    fn process_init_escrow(
        accounts: &[AccountInfo],
        amount: u64,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let initializer = next_account_info(account_info_iter)?;

        if !initializer.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }
        
        let temp_token_account = next_account_info(account_info_iter)?;

        let withdrawer_account = next_account_info(account_info_iter)?;

        let escrow_account = next_account_info(account_info_iter)?;
        let rent = &Rent::from_account_info(next_account_info(account_info_iter)?)?;

        if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
            return Err(EscrowError::NotRentExempt.into());
        }

        let mut escrow_info = Escrow::unpack_unchecked(&escrow_account.try_borrow_data()?)?;
        if escrow_info.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        escrow_info.is_initialized = true;
        escrow_info.initializer_pubkey = *initializer.key;
        escrow_info.temp_token_account_pubkey = *temp_token_account.key;
        escrow_info.withdrawer_pubkey = *withdrawer_account.key;
        escrow_info.deposited_amount = amount;

        Escrow::pack(escrow_info, &mut escrow_account.try_borrow_mut_data()?)?;
        let (pda, _nonce) = Pubkey::find_program_address(&[b"escrow"], program_id);

        let token_program = next_account_info(account_info_iter)?;
        let owner_change_ix = spl_token::instruction::set_authority(
            token_program.key,
            temp_token_account.key,
            Some(&pda),
            spl_token::instruction::AuthorityType::AccountOwner,
            initializer.key,
            &[&initializer.key],
        )?;

        msg!("Calling the token program to transfer token account ownership...");
        invoke(
            &owner_change_ix,
            &[
                temp_token_account.clone(),
                initializer.clone(),
                token_program.clone(),
            ],
        )?;
        Ok(())
    }

    fn process_withdraw(
        accounts: &[AccountInfo],
        amount_to_withdraw: u64,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let taker = next_account_info(account_info_iter)?;

        if !taker.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let takers_token_to_receive_account = next_account_info(account_info_iter)?;

        let pdas_temp_token_account = next_account_info(account_info_iter)?;
        let pdas_temp_token_account_info =
            TokenAccount::unpack(&pdas_temp_token_account.try_borrow_data()?)?;
        let (pda, nonce) = Pubkey::find_program_address(&[b"escrow"], program_id);

        if amount_to_withdraw > pdas_temp_token_account_info.amount {
            return Err(EscrowError::ExpectedAmountMismatch.into());
        }

        let initializers_main_account = next_account_info(account_info_iter)?;
        let escrow_account = next_account_info(account_info_iter)?;

        let mut escrow_info = Escrow::unpack(&escrow_account.try_borrow_data()?)?;

        if escrow_info.temp_token_account_pubkey != *pdas_temp_token_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_info.initializer_pubkey != *initializers_main_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if escrow_info.withdrawer_pubkey != *taker.key {
            return Err(ProgramError::InvalidAccountData);
        }

        let token_program = next_account_info(account_info_iter)?;

        let pda_account = next_account_info(account_info_iter)?;


        // withdraw amount check
        // already checked in line 115 
        // if amount > escrow_info.deposited_amount{
        //     return Err(ProgramError::InvalidAccountData);
        // }
        // escrow_info.deposited_amount or pdas_temp_token_account_info.amount can be used i think. Same huna parne ho as per my code.
        if amount_to_withdraw < escrow_info.deposited_amount{
            let remaining_amount = escrow_info.deposited_amount - amount_to_withdraw;
            let transfer_to_taker_ix = spl_token::instruction::transfer(
                token_program.key,
                pdas_temp_token_account.key,
                takers_token_to_receive_account.key,
                &pda,
                &[&pda],
                amount_to_withdraw,
            )?;
            msg!("Calling the token program to transfer {} tokens to the taker...", amount_to_withdraw);
            invoke_signed(
                &transfer_to_taker_ix,
                &[
                    pdas_temp_token_account.clone(),
                    takers_token_to_receive_account.clone(),
                    pda_account.clone(),
                    token_program.clone(),
                ],
                &[&[&b"escrow"[..], &[nonce]]],
            )?;
            // store new info into escro account
            escrow_info.deposited_amount = remaining_amount;
            Escrow::pack(escrow_info, &mut escrow_account.try_borrow_mut_data()?)?;
        }
        else{
            let transfer_to_taker_ix = spl_token::instruction::transfer(
                token_program.key,
                pdas_temp_token_account.key,
                takers_token_to_receive_account.key,
                &pda,
                &[&pda],
                pdas_temp_token_account_info.amount,
            )?;
            msg!("Calling the token program to transfer all tokens to the taker...");
            invoke_signed(
                &transfer_to_taker_ix,
                &[
                    pdas_temp_token_account.clone(),
                    takers_token_to_receive_account.clone(),
                    pda_account.clone(),
                    token_program.clone(),
                ],
                &[&[&b"escrow"[..], &[nonce]]],
            )?;
            let close_pdas_temp_acc_ix = spl_token::instruction::close_account(
                token_program.key,
                pdas_temp_token_account.key,
                initializers_main_account.key,
                &pda,
                &[&pda],
            )?;
            msg!("Calling the token program to close pda's temp account...");
            invoke_signed(
                &close_pdas_temp_acc_ix,
                &[
                    pdas_temp_token_account.clone(),
                    initializers_main_account.clone(),
                    pda_account.clone(),
                    token_program.clone(),
                ],
                &[&[&b"escrow"[..], &[nonce]]],
            )?;

            msg!("Closing the escrow account...");
            **initializers_main_account.try_borrow_mut_lamports()? = initializers_main_account
                .lamports()
                .checked_add(escrow_account.lamports())
                .ok_or(EscrowError::AmountOverflow)?;
            **escrow_account.try_borrow_mut_lamports()? = 0;
            *escrow_account.try_borrow_mut_data()? = &mut [];
        }

        Ok(())
    }
}
