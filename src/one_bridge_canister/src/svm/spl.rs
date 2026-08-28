use solana_instruction::{AccountMeta, Instruction};

use super::types::Pubkey;

const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey =
    Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SYSTEM_PROGRAM_ID: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

pub fn get_associated_token_address(
    wallet_address: &Pubkey,
    token_mint_address: &Pubkey,
    token_program_id: &Pubkey,
) -> Pubkey {
    Pubkey::find_program_address(
        &[
            wallet_address.as_ref(),
            token_program_id.as_ref(),
            token_mint_address.as_ref(),
        ],
        &ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    .0
}

pub fn create_associated_token_account_idempotent(
    funding_address: &Pubkey,
    wallet_address: &Pubkey,
    token_mint_address: &Pubkey,
    token_program_id: &Pubkey,
) -> Instruction {
    let associated_account =
        get_associated_token_address(wallet_address, token_mint_address, token_program_id);
    Instruction {
        program_id: ASSOCIATED_TOKEN_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*funding_address, true),
            AccountMeta::new(associated_account, false),
            AccountMeta::new_readonly(*wallet_address, false),
            AccountMeta::new_readonly(*token_mint_address, false),
            AccountMeta::new_readonly(SYSTEM_PROGRAM_ID, false),
            AccountMeta::new_readonly(*token_program_id, false),
        ],
        // Associated Token Account instruction 1 is CreateIdempotent.
        data: vec![1],
    }
}

pub fn system_transfer_instruction(
    from_address: &Pubkey,
    to_address: &Pubkey,
    lamports: u64,
) -> Instruction {
    let mut data = Vec::with_capacity(12);
    // bincode encoding of SystemInstruction::Transfer, the third enum variant.
    data.extend_from_slice(&2_u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![
            AccountMeta::new(*from_address, true),
            AccountMeta::new(*to_address, false),
        ],
        data,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn transfer_checked_instruction(
    token_program_id: &Pubkey,
    source_pubkey: &Pubkey,
    mint_pubkey: &Pubkey,
    destination_pubkey: &Pubkey,
    authority_pubkey: &Pubkey, // The source account's owner/delegate.
    signer_pubkeys: &[&Pubkey],
    amount: u64,
    decimals: u8,
) -> Instruction {
    let mut data = Vec::with_capacity(10);
    // SPL token program "TransferChecked" instruction
    data.push(12);
    data.extend_from_slice(&amount.to_le_bytes());
    data.push(decimals);
    let mut accounts = Vec::with_capacity(4 + signer_pubkeys.len());
    accounts.push(AccountMeta::new(*source_pubkey, false));
    accounts.push(AccountMeta::new_readonly(*mint_pubkey, false));
    accounts.push(AccountMeta::new(*destination_pubkey, false));
    accounts.push(AccountMeta::new_readonly(
        *authority_pubkey,
        signer_pubkeys.is_empty(),
    ));
    for signer_pubkey in signer_pubkeys.iter() {
        accounts.push(AccountMeta::new_readonly(**signer_pubkey, true));
    }
    Instruction {
        program_id: *token_program_id,
        accounts,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_transfer_uses_the_solana_wire_layout() {
        let from = Pubkey::new_from_array([1; 32]);
        let to = Pubkey::new_from_array([2; 32]);
        let instruction = system_transfer_instruction(&from, &to, 42);

        assert_eq!(instruction.program_id, SYSTEM_PROGRAM_ID);
        assert_eq!(instruction.accounts.len(), 2);
        assert!(instruction.accounts[0].is_signer);
        let mut expected = 2_u32.to_le_bytes().to_vec();
        expected.extend_from_slice(&42_u64.to_le_bytes());
        assert_eq!(instruction.data, expected);
    }

    #[test]
    fn associated_account_creation_is_idempotent() {
        let payer = Pubkey::new_from_array([1; 32]);
        let wallet = Pubkey::new_from_array([2; 32]);
        let mint = Pubkey::new_from_array([3; 32]);
        let token_program = Pubkey::new_from_array([4; 32]);
        let instruction =
            create_associated_token_account_idempotent(&payer, &wallet, &mint, &token_program);

        assert_eq!(instruction.program_id, ASSOCIATED_TOKEN_PROGRAM_ID);
        assert_eq!(instruction.accounts.len(), 6);
        assert_eq!(instruction.data, [1]);
        assert_eq!(
            instruction.accounts[1].pubkey,
            get_associated_token_address(&wallet, &mint, &token_program)
        );
    }
}
