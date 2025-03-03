//! Program state processor

use {
    solana_program::{
        account_info::{next_account_info, AccountInfo},
        entrypoint::ProgramResult,
        msg,
        program_error::ProgramError,
        program_option::COption,
        pubkey::Pubkey,
    },
    spl_pod::optional_keys::OptionalNonZeroPubkey,
    spl_token_2022::{
        extension::{
            metadata_pointer::MetadataPointer, BaseStateWithExtensions, StateWithExtensions,
        },
        state::Mint,
    },
    spl_token_group_interface::{
        error::TokenGroupError,
        instruction::{InitializeGroup, TokenGroupInstruction},
        state::{TokenGroup, TokenGroupMember},
    },
    spl_token_metadata_interface::state::TokenMetadata,
    spl_type_length_value::state::TlvStateMut,
};

fn check_update_authority(
    update_authority_info: &AccountInfo,
    expected_update_authority: &OptionalNonZeroPubkey,
) -> ProgramResult {
    if !update_authority_info.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let update_authority = Option::<Pubkey>::from(*expected_update_authority)
        .ok_or(TokenGroupError::ImmutableGroup)?;
    if update_authority != *update_authority_info.key {
        return Err(TokenGroupError::IncorrectUpdateAuthority.into());
    }
    Ok(())
}

/// Checks that a mint is valid and contains metadata.
fn check_mint_and_metadata(
    mint_info: &AccountInfo,
    mint_authority_info: &AccountInfo,
) -> ProgramResult {
    let mint_data = mint_info.try_borrow_data()?;
    let mint = StateWithExtensions::<Mint>::unpack(&mint_data)?;

    // 确保 mint_authority_info 是一个有效签名者，即调用该函数的账户必须是 Mint 账户的实际管理者。
    if !mint_authority_info.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    if mint.base.mint_authority.as_ref() != COption::Some(mint_authority_info.key) {
        return Err(TokenGroupError::IncorrectMintAuthority.into());
    }

    let metadata_pointer = mint.get_extension::<MetadataPointer>()?;
    let metadata_pointer_address = Option::<Pubkey>::from(metadata_pointer.metadata_address);

    // If the metadata is inside the mint (Token2022), make sure it contains
    // valid TokenMetadata
    // 确保 Token 2022 Mint 包含 Metadata
    if metadata_pointer_address == Some(*mint_info.key) {
        mint.get_variable_len_extension::<TokenMetadata>()?;
    }

    Ok(())
}

/// Processes an [InitializeGroup](enum.GroupInterfaceInstruction.html)
/// instruction to initialize a collection.
pub fn process_initialize_collection(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: InitializeGroup,
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let collection_info = next_account_info(account_info_iter)?;
    let mint_info = next_account_info(account_info_iter)?;
    let mint_authority_info = next_account_info(account_info_iter)?;
    // 验证 Mint 账户及其 Metadata
    check_mint_and_metadata(mint_info, mint_authority_info)?;

    // Initialize the collection
    // 创建 TokenGroup 并存入 Collection
    let mut buffer = collection_info.try_borrow_mut_data()?;
    let mut state = TlvStateMut::unpack(&mut buffer)?;
    let (collection, _) = state.init_value::<TokenGroup>(false)?;
    *collection = TokenGroup::new(mint_info.key, data.update_authority, data.max_size.into());

    Ok(())
}

/// Processes an [InitializeMember](enum.GroupInterfaceInstruction.html)
/// instruction
/// ✅ 核心作用：
// 	1.	解析 accounts，确保 mint_info 是有效 Token Mint，且有元数据。
// 	2.	确保 collection_update_authority_info 是 Collection 的合法管理者。
// 	3.	防止 Collection 账户本身成为成员（错误检查）。
// 	4.	增加 Collection 成员总数 并存入 collection_info。
// 	5.	初始化 TokenGroupMember 并存入 member_info，完成成员注册。

// ✅ 适用场景：
// 	•	NFT/SFT 集合：将 NFT/SFT 加入某个 Collection（类似 Metaplex）。
// 	•	Token Group（SPL Token 2022）：管理代币组，允许某个 Token 归属于特定分组。
// 	•	链上资产管理：创建分类账户，分组管理不同的资产。
pub fn process_initialize_collection_member(
    _program_id: &Pubkey,
    accounts: &[AccountInfo],
) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let member_info = next_account_info(account_info_iter)?;
    let mint_info = next_account_info(account_info_iter)?;
    let mint_authority_info = next_account_info(account_info_iter)?;
    let collection_info = next_account_info(account_info_iter)?;
    let collection_update_authority_info = next_account_info(account_info_iter)?;

    check_mint_and_metadata(mint_info, mint_authority_info)?;

    if member_info.key == collection_info.key {
        return Err(TokenGroupError::MemberAccountIsGroupAccount.into());
    }

    let mut buffer = collection_info.try_borrow_mut_data()?;
    let mut state = TlvStateMut::unpack(&mut buffer)?;
    let collection = state.get_first_value_mut::<TokenGroup>()?;
    // 检查 Update Authority（管理者权限）
    check_update_authority(
        collection_update_authority_info,
        &collection.update_authority,
    )?;
    // 增加集合的成员数量
    let member_number = collection.increment_size()?;

    let mut buffer = member_info.try_borrow_mut_data()?;
    let mut state = TlvStateMut::unpack(&mut buffer)?;
    // 初始化 Member 账户
    // This program uses `allow_repetition: true` because the same mint can be
    // a member of multiple collections.
    let (member, _) = state.init_value::<TokenGroupMember>(/* allow_repetition */ true)?;
    *member = TokenGroupMember::new(mint_info.key, collection_info.key, member_number);

    Ok(())
}

/// Processes an `SplTokenGroupInstruction`
pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], input: &[u8]) -> ProgramResult {
    let instruction = TokenGroupInstruction::unpack(input)?;
    match instruction {
        // 初始化 Token 组
        TokenGroupInstruction::InitializeGroup(data) => {
            msg!("Instruction: InitializeCollection");
            process_initialize_collection(program_id, accounts, data)
        }
        // 更新 Token 组的最大大小
        TokenGroupInstruction::UpdateGroupMaxSize(data) => {
            msg!("Instruction: UpdateCollectionMaxSize");
            // Same functionality as the example program
            spl_token_group_example::processor::process_update_group_max_size(
                program_id, accounts, data,
            )
        }
        // 更新 Token 组的管理员
        TokenGroupInstruction::UpdateGroupAuthority(data) => {
            msg!("Instruction: UpdateCollectionAuthority");
            // Same functionality as the example program
            spl_token_group_example::processor::process_update_group_authority(
                program_id, accounts, data,
            )
        }
        // 初始化 Token 组成员
        TokenGroupInstruction::InitializeMember(_) => {
            msg!("Instruction: InitializeCollectionMember");
            process_initialize_collection_member(program_id, accounts)
        }
    }
}
