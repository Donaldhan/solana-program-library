//! Program state processor

use {
    crate::{
        constraints::{SwapConstraints, SWAP_CONSTRAINTS},
        curve::{
            base::SwapCurve,
            calculator::{RoundDirection, TradeDirection},
            fees::Fees,
        },
        error::SwapError,
        instruction::{
            DepositAllTokenTypes, DepositSingleTokenTypeExactAmountIn, Initialize, Swap,
            SwapInstruction, WithdrawAllTokenTypes, WithdrawSingleTokenTypeExactAmountOut,
        },
        state::{SwapState, SwapV1, SwapVersion},
    },
    num_traits::FromPrimitive,
    solana_program::{
        account_info::{next_account_info, AccountInfo},
        clock::Clock,
        decode_error::DecodeError,
        entrypoint::ProgramResult,
        instruction::Instruction,
        msg,
        program::invoke_signed,
        program_error::{PrintProgramError, ProgramError},
        program_option::COption,
        pubkey::Pubkey,
        sysvar::Sysvar,
    },
    spl_token_2022::{
        check_spl_token_program_account,
        error::TokenError,
        extension::{
            mint_close_authority::MintCloseAuthority, transfer_fee::TransferFeeConfig,
            BaseStateWithExtensions, StateWithExtensions,
        },
        state::{Account, Mint},
    },
    std::{convert::TryInto, error::Error},
};

/// Program state handler.
pub struct Processor {}
impl Processor {
    /// Unpacks a spl_token `Account`.
    pub fn unpack_token_account(
        account_info: &AccountInfo,
        token_program_id: &Pubkey,
    ) -> Result<Account, SwapError> {
        if account_info.owner != token_program_id
            && check_spl_token_program_account(account_info.owner).is_err()
        {
            Err(SwapError::IncorrectTokenProgramId)
        } else {
            StateWithExtensions::<Account>::unpack(&account_info.data.borrow())
                .map(|a| a.base)
                .map_err(|_| SwapError::ExpectedAccount)
        }
    }

    /// Unpacks a spl_token `Mint`.
    pub fn unpack_mint(
        account_info: &AccountInfo,
        token_program_id: &Pubkey,
    ) -> Result<Mint, SwapError> {
        if account_info.owner != token_program_id
            && check_spl_token_program_account(account_info.owner).is_err()
        {
            Err(SwapError::IncorrectTokenProgramId)
        } else {
            StateWithExtensions::<Mint>::unpack(&account_info.data.borrow())
                .map(|m| m.base)
                .map_err(|_| SwapError::ExpectedMint)
        }
    }

    /// Unpacks a spl_token `Mint` with extension data
    pub fn unpack_mint_with_extensions<'a>(
        account_data: &'a [u8],
        owner: &Pubkey,
        token_program_id: &Pubkey,
    ) -> Result<StateWithExtensions<'a, Mint>, SwapError> {
        if owner != token_program_id && check_spl_token_program_account(owner).is_err() {
            Err(SwapError::IncorrectTokenProgramId)
        } else {
            StateWithExtensions::<Mint>::unpack(account_data).map_err(|_| SwapError::ExpectedMint)
        }
    }

    /// Calculates the authority id by generating a program address.
    pub fn authority_id(
        program_id: &Pubkey,
        my_info: &Pubkey,
        bump_seed: u8,
    ) -> Result<Pubkey, SwapError> {
        Pubkey::create_program_address(&[&my_info.to_bytes()[..32], &[bump_seed]], program_id)
            .or(Err(SwapError::InvalidProgramAddress))
    }

    /// Issue a spl_token `Burn` instruction.
    /// 这个 token_burn 函数实现了一个代币燃烧操作，即从指定的账户（burn_account）销毁一定数量的代币。具体步骤如下：
	// 1.	生成与交换合约相关的签名密钥（authority_signature_seeds）。
	// 2.	创建燃烧指令，指定销毁代币的账户、代币铸造账户和授权账户。
	// 3.	使用 invoke_signed_wrapper 执行燃烧操作，并确保燃烧操作得到授权。
    pub fn token_burn<'a>(
        swap: &Pubkey,
        token_program: AccountInfo<'a>,
        burn_account: AccountInfo<'a>,
        mint: AccountInfo<'a>,
        authority: AccountInfo<'a>,
        bump_seed: u8,
        amount: u64,
    ) -> Result<(), ProgramError> {
        // 生成签名密钥
        let swap_bytes = swap.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[bump_seed]];
        let signers = &[&authority_signature_seeds[..]];
        // 创建燃烧指令
        let ix = spl_token_2022::instruction::burn(
            token_program.key,
            burn_account.key,
            mint.key,
            authority.key,
            &[],
            amount,
        )?;

        invoke_signed_wrapper::<TokenError>(
            &ix,
            &[burn_account, mint, authority, token_program],
            signers,
        )
    }

    /// Issue a spl_token `MintTo` instruction.
    /// 	该函数 使用 PDA (Program Derived Address) 作为 mint 账户的 authority 来铸造 SPL 代币。
    // •	核心步骤：
    // 1.	计算 PDA 签名种子 (swap_bytes + bump_seed)。
    // 2.	通过 spl_token_2022::instruction::mint_to 构造 MintTo 指令。
    // 3.	使用 invoke_signed_wrapper 调用该指令，并使用 PDA 进行授权签名。
    // •	适用于 自动化代币铸造场景，如 AMM (自动做市商)、稳定币协议、流动性质押等。
    pub fn token_mint_to<'a>(
        swap: &Pubkey,
        token_program: AccountInfo<'a>,
        mint: AccountInfo<'a>,
        destination: AccountInfo<'a>,
        authority: AccountInfo<'a>,
        bump_seed: u8,
        amount: u64,
    ) -> Result<(), ProgramError> {
        let swap_bytes = swap.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[bump_seed]];
        let signers = &[&authority_signature_seeds[..]];
        let ix = spl_token_2022::instruction::mint_to(
            token_program.key,
            mint.key,
            destination.key,
            authority.key,
            &[],
            amount,
        )?;

        invoke_signed_wrapper::<TokenError>(
            &ix,
            &[mint, destination, authority, token_program],
            signers,
        )
    }
    // 通过 SPL Token 进行代币转账的功能，使用了 spl_token_2022 库中的 transfer_checked 指令。具体功能是发起一个转账请求，并使用 invoke_signed_wrapper 进行签名验证
    /// Issue a spl_token `Transfer` instruction.
    /// 	•	swap: &Pubkey：表示交换合约的公钥。
    // •	token_program: AccountInfo<'a>：表示代币程序的账户信息。
    // •	source: AccountInfo<'a>：表示源账户，即从中转出代币的账户。
    // •	mint: AccountInfo<'a>：表示代币的 mint 地址（代币的类型标识符）。
    // •	destination: AccountInfo<'a>：目标账户，即接收代币的账户。
    // •	authority: AccountInfo<'a>：代币转账的授权账户，一般是 swap 合约的签名者。
    // •	bump_seed: u8：用于生成签名授权种子的 bump，是为了确保合约账户签名的唯一性。
    // •	amount: u64：要转账的代币数量。
    // •	decimals: u8：代币的精度（即每个代币的最小单位的位数）。
    #[allow(clippy::too_many_arguments)]
    pub fn token_transfer<'a>(
        swap: &Pubkey,
        token_program: AccountInfo<'a>,
        source: AccountInfo<'a>,
        mint: AccountInfo<'a>,
        destination: AccountInfo<'a>,
        authority: AccountInfo<'a>,
        bump_seed: u8,
        amount: u64,
        decimals: u8,
    ) -> Result<(), ProgramError> {
        let swap_bytes = swap.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[bump_seed]];
        // signers：表示签名的数组，包含签名种子 authority_signature_seeds，用于后续验证签名。
        // •	authority_signature_seeds：是由 swap 公钥的字节和 bump_seed 组合而成的签名种子，确保每次生成的签名都是唯一的。
        // •	signers：是包含签名种子的数组，invoke_signed 函数用它来验证交易是否由授权者签署。
        // •	签名验证：通过验证签名和交易数据的完整性，Solana 确保了每个交易的合法性和安全性。
        let signers = &[&authority_signature_seeds[..]];
        //     spl_token_2022::instruction::transfer_checked：构建一个 transfer_checked 指令，它是 SPL Token 2022 版的转账指令。
        // •	token_program.key：代币程序的公钥。
        // •	source.key：源账户的公钥。
        // •	mint.key：代币 mint 的公钥。
        // •	destination.key：目标账户的公钥。
        // •	authority.key：授权账户的公钥。
        // •	[]：空的签名数组，意味着没有额外的签名。
        // •	amount：要转账的金额。
        // •	decimals：代币的精度。
        let ix = spl_token_2022::instruction::transfer_checked(
            token_program.key,
            source.key,
            mint.key,
            destination.key,
            authority.key,
            &[],
            amount,
            decimals,
        )?;
        // •	invoke_signed_wrapper::<TokenError>：用于执行带签名验证的交易。
        // •	&ix：代币转账指令。
        // •	[source, mint, destination, authority, token_program]：参与交易的账户列表，必须是传入的账户信息。
        // •	signers：签名者信息，使用签名种子来验证交易。

        invoke_signed_wrapper::<TokenError>(
            &ix,
            &[source, mint, destination, authority, token_program],
            signers,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn check_accounts(
        token_swap: &dyn SwapState,
        program_id: &Pubkey,
        swap_account_info: &AccountInfo,
        authority_info: &AccountInfo,
        token_a_info: &AccountInfo,
        token_b_info: &AccountInfo,
        pool_mint_info: &AccountInfo,
        pool_token_program_info: &AccountInfo,
        user_token_a_info: Option<&AccountInfo>,
        user_token_b_info: Option<&AccountInfo>,
        pool_fee_account_info: Option<&AccountInfo>,
    ) -> ProgramResult {
        if swap_account_info.owner != program_id {
            return Err(ProgramError::IncorrectProgramId);
        }
        if *authority_info.key
            != Self::authority_id(program_id, swap_account_info.key, token_swap.bump_seed())?
        {
            return Err(SwapError::InvalidProgramAddress.into());
        }
        if *token_a_info.key != *token_swap.token_a_account() {
            return Err(SwapError::IncorrectSwapAccount.into());
        }
        if *token_b_info.key != *token_swap.token_b_account() {
            return Err(SwapError::IncorrectSwapAccount.into());
        }
        if *pool_mint_info.key != *token_swap.pool_mint() {
            return Err(SwapError::IncorrectPoolMint.into());
        }
        if *pool_token_program_info.key != *token_swap.token_program_id() {
            return Err(SwapError::IncorrectTokenProgramId.into());
        }
        // •	如果传入了 user_token_a_info 或 user_token_b_info，检查这些账户是否与 token_a_info 或 token_b_info 匹配。
        // •	如果是相同的账户，返回错误 InvalidInput，表示用户不应将自己持有的代币账户作为存入账户。
        if let Some(user_token_a_info) = user_token_a_info {
            if token_a_info.key == user_token_a_info.key {
                return Err(SwapError::InvalidInput.into());
            }
        }
        if let Some(user_token_b_info) = user_token_b_info {
            if token_b_info.key == user_token_b_info.key {
                return Err(SwapError::InvalidInput.into());
            }
        }
        if let Some(pool_fee_account_info) = pool_fee_account_info {
            if *pool_fee_account_info.key != *token_swap.pool_fee_account() {
                return Err(SwapError::IncorrectFeeAccount.into());
            }
        }
        Ok(())
    }

    /// Processes an [Initialize](enum.Instruction.html).
    /// process_initialize 主要用于 初始化一个 Swap (流动性池) 交易合约，它属于 Solana 上的去中心化交易 (DEX) 或流动性池 (AMM, Automated Market Maker) 逻辑，符合 SPL Token 交换协议。
    // 它的作用是：
    // 1.	验证账户信息（确保账户权限和初始状态正确）。
    // 2.	验证交易对（token A 和 token B）是否有效，并检查流动性池是否已初始化。
    // 3.	计算并铸造流动性池 (LP) 代币，用于代表流动性提供者的权益。
    // 4.	存储流动性池的 Swap 信息，供后续交换交易使用。

    // •	program_id：当前合约的 ID，确保调用的是正确的合约。
    // •	fees：用于设置 Swap 手续费，比如流动性提供者 (LP) 费用、协议费用等。
    // •	swap_curve：用于控制 Swap 交易价格的数学模型，通常是 恒定乘积曲线 (x * y = k) 或其他曲线模型。
    // •	accounts：包含多个账户（Swap 账户、授权账户、代币账户、流动性池账户等）。
    // •	swap_constraints (可选)：用于限制某些 Swap 规则，例如允许的交易对或费用上限。
    pub fn process_initialize(
        program_id: &Pubkey,
        fees: Fees,
        swap_curve: SwapCurve,
        accounts: &[AccountInfo],
        swap_constraints: &Option<SwapConstraints>,
    ) -> ProgramResult {
        // •	swap_info：流动性池账户（Swap 账户）。
        // •	authority_info：Swap 合约的 PDA (Program Derived Address)，用于管理 Swap 池。
        // •	token_a_info / token_b_info：要交换的两个代币账户 (Token A 和 Token B)。
        // •	pool_mint_info：流动性池代币（LP 代币）账户。
        // •	fee_account_info：Swap 交易费用账户。
        // •	destination_info：接收流动性池代币的账户。
        // •	pool_token_program_info：SPL 代币合约地址。
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let token_a_info = next_account_info(account_info_iter)?;
        let token_b_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let fee_account_info = next_account_info(account_info_iter)?;
        let destination_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;

        // 检查 Swap 是否已被初始化
        let token_program_id = *pool_token_program_info.key;
        if SwapVersion::is_initialized(&swap_info.data.borrow()) {
            return Err(SwapError::AlreadyInUse.into());
        }
        // 计算 PDA (Program Derived Address)
        let (swap_authority, bump_seed) =
            Pubkey::find_program_address(&[&swap_info.key.to_bytes()], program_id);
        if *authority_info.key != swap_authority {
            return Err(SwapError::InvalidProgramAddress.into());
        }
        // 解析并检查代币账户
        // 这里解析 Token A、Token B、费用账户和 LP 代币接收账户的状态。
        let token_a = Self::unpack_token_account(token_a_info, &token_program_id)?;
        let token_b = Self::unpack_token_account(token_b_info, &token_program_id)?;
        let fee_account = Self::unpack_token_account(fee_account_info, &token_program_id)?;
        let destination = Self::unpack_token_account(destination_info, &token_program_id)?;
        // 解析并检查代币账户
        // 解析 LP 代币 (流动性池代币) 的 Mint 账户，并检查 Mint 账户不能有 close_authority，确保它不会被关闭。
        let pool_mint = {
            let pool_mint_data = pool_mint_info.data.borrow();
            let pool_mint = Self::unpack_mint_with_extensions(
                &pool_mint_data,
                pool_mint_info.owner,
                &token_program_id,
            )?;
            if let Ok(extension) = pool_mint.get_extension::<MintCloseAuthority>() {
                let close_authority: Option<Pubkey> = extension.close_authority.into();
                if close_authority.is_some() {
                    return Err(SwapError::InvalidCloseAuthority.into());
                }
            }
            pool_mint.base
        };
        if *authority_info.key != token_a.owner {
            return Err(SwapError::InvalidOwner.into());
        }
        if *authority_info.key != token_b.owner {
            return Err(SwapError::InvalidOwner.into());
        }
        if *authority_info.key == destination.owner {
            return Err(SwapError::InvalidOutputOwner.into());
        }
        if *authority_info.key == fee_account.owner {
            return Err(SwapError::InvalidOutputOwner.into());
        }
        if COption::Some(*authority_info.key) != pool_mint.mint_authority {
            return Err(SwapError::InvalidOwner.into());
        }

        if token_a.mint == token_b.mint {
            return Err(SwapError::RepeatedMint.into());
        }
        swap_curve
            .calculator
            .validate_supply(token_a.amount, token_b.amount)?;
        if token_a.delegate.is_some() {
            return Err(SwapError::InvalidDelegate.into());
        }
        if token_b.delegate.is_some() {
            return Err(SwapError::InvalidDelegate.into());
        }
        if token_a.close_authority.is_some() {
            return Err(SwapError::InvalidCloseAuthority.into());
        }
        if token_b.close_authority.is_some() {
            return Err(SwapError::InvalidCloseAuthority.into());
        }

        if pool_mint.supply != 0 {
            return Err(SwapError::InvalidSupply.into());
        }
        if pool_mint.freeze_authority.is_some() {
            return Err(SwapError::InvalidFreezeAuthority.into());
        }
        if *pool_mint_info.key != fee_account.mint {
            return Err(SwapError::IncorrectPoolMint.into());
        }

        if let Some(swap_constraints) = swap_constraints {
            let owner_key = swap_constraints
                .owner_key
                .unwrap()
                .parse::<Pubkey>()
                .map_err(|_| SwapError::InvalidOwner)?;
            if fee_account.owner != owner_key {
                return Err(SwapError::InvalidOwner.into());
            }
            swap_constraints.validate_curve(&swap_curve)?;
            swap_constraints.validate_fees(&fees)?;
        }
        fees.validate()?;
        swap_curve.calculator.validate()?;

        let initial_amount = swap_curve.calculator.new_pool_supply();
        // 计算初始的流动性池代币数量，然后铸造 LP 代币到 destination_info (通常是流动性提供者的账户)。
        Self::token_mint_to(
            swap_info.key,
            pool_token_program_info.clone(),
            pool_mint_info.clone(),
            destination_info.clone(),
            authority_info.clone(),
            bump_seed,
            to_u64(initial_amount)?,
        )?;
        // 保存流动性池的状态，包括：
        // •	Token A / Token B 账户地址
        // •	LP 代币池
        // •	交易费率
        // •	Swap 交易曲线
        // •	是否已初始化
        let obj = SwapVersion::SwapV1(SwapV1 {
            is_initialized: true,
            bump_seed,
            token_program_id,
            token_a: *token_a_info.key,
            token_b: *token_b_info.key,
            pool_mint: *pool_mint_info.key,
            token_a_mint: token_a.mint,
            token_b_mint: token_b.mint,
            pool_fee_account: *fee_account_info.key,
            fees,
            swap_curve,
        });
        SwapVersion::pack(obj, &mut swap_info.data.borrow_mut())?;
        Ok(())
    }

    /// Processes an [Swap](enum.Instruction.html).
    /// 该函数 process_swap 主要负责处理代币交换请求，其核心逻辑包括：
    // •	验证账户参数是否合法
    // •	计算实际的交换数量（扣除转账费用）
    // •	通过交换曲线计算最终的兑换结果
    // •	处理交易费用（包含流动性提供者的费用及协议费）
    // •	进行代币转移
    pub fn process_swap(
        program_id: &Pubkey,
        amount_in: u64,
        minimum_amount_out: u64,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let source_info = next_account_info(account_info_iter)?;
        let swap_source_info = next_account_info(account_info_iter)?;
        let swap_destination_info = next_account_info(account_info_iter)?;
        let destination_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let pool_fee_account_info = next_account_info(account_info_iter)?;
        let source_token_mint_info = next_account_info(account_info_iter)?;
        let destination_token_mint_info = next_account_info(account_info_iter)?;
        let source_token_program_info = next_account_info(account_info_iter)?;
        let destination_token_program_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;

        //     确保 swap_info 账户由 program_id 所管理。
        // •	解析 swap_info 数据以获取 token_swap 结构体。
        if swap_info.owner != program_id {
            return Err(ProgramError::IncorrectProgramId);
        }
        let token_swap = SwapVersion::unpack(&swap_info.data.borrow())?;

        // 检查 authority_info 是否与 swap_info 关联的授权账户匹配。
        if *authority_info.key
            != Self::authority_id(program_id, swap_info.key, token_swap.bump_seed())?
        {
            return Err(SwapError::InvalidProgramAddress.into());
        }
        // 确保 swap_source_info 和 swap_destination_info 属于交换池。
        if !(*swap_source_info.key == *token_swap.token_a_account()
            || *swap_source_info.key == *token_swap.token_b_account())
        {
            return Err(SwapError::IncorrectSwapAccount.into());
        }
        if !(*swap_destination_info.key == *token_swap.token_a_account()
            || *swap_destination_info.key == *token_swap.token_b_account())
        {
            return Err(SwapError::IncorrectSwapAccount.into());
        }
        if *swap_source_info.key == *swap_destination_info.key {
            return Err(SwapError::InvalidInput.into());
        }
        if swap_source_info.key == source_info.key {
            return Err(SwapError::InvalidInput.into());
        }
        if swap_destination_info.key == destination_info.key {
            return Err(SwapError::InvalidInput.into());
        }
        if *pool_mint_info.key != *token_swap.pool_mint() {
            return Err(SwapError::IncorrectPoolMint.into());
        }
        if *pool_fee_account_info.key != *token_swap.pool_fee_account() {
            return Err(SwapError::IncorrectFeeAccount.into());
        }
        if *pool_token_program_info.key != *token_swap.token_program_id() {
            return Err(SwapError::IncorrectTokenProgramId.into());
        }

        let source_account =
            Self::unpack_token_account(swap_source_info, token_swap.token_program_id())?;
        let dest_account =
            Self::unpack_token_account(swap_destination_info, token_swap.token_program_id())?;
        let pool_mint = Self::unpack_mint(pool_mint_info, token_swap.token_program_id())?;

        // Take transfer fees into account for actual amount transferred in
        //     解析源代币的 mint 信息，检查是否有 TransferFeeConfig（即该代币是否有转账费用）。
        // •	如果有，则计算扣除转账费后的 actual_amount_in，否则 actual_amount_in = amount_in。
        let actual_amount_in = {
            let source_mint_data = source_token_mint_info.data.borrow();
            let source_mint = Self::unpack_mint_with_extensions(
                &source_mint_data,
                source_token_mint_info.owner,
                token_swap.token_program_id(),
            )?;
            // 1.	尝试从 source_mint 获取转账手续费配置 (TransferFeeConfig)。
            // 2.	如果成功获取到配置，则根据当前 epoch 和转账金额 amount_in 计算应收取的手续费，并从 amount_in 中扣除相应的手续费。
            // 3.	如果获取手续费配置失败，则直接返回原始金额 amount_in，即不进行手续费扣除。
            if let Ok(transfer_fee_config) = source_mint.get_extension::<TransferFeeConfig>() {
                amount_in.saturating_sub(
                    transfer_fee_config
                        .calculate_epoch_fee(Clock::get()?.epoch, amount_in)
                        .ok_or(SwapError::FeeCalculationFailure)?,
                )
            } else {
                amount_in
            }
        };

        // Calculate the trade amounts
        // 确定交易方向，是从 Token A 换成 Token B，还是从 Token B 换成 Token A。
        let trade_direction = if *swap_source_info.key == *token_swap.token_a_account() {
            TradeDirection::AtoB
        } else {
            TradeDirection::BtoA
        };
        // 通过 swap_curve 计算 source_amount_swapped 和 destination_amount_swapped，即：
        // •	交易后源代币账户的余额
        // •	交易后目标代币账户的余额
        let result = token_swap
            .swap_curve()
            .swap(
                u128::from(actual_amount_in),
                u128::from(source_account.amount),
                u128::from(dest_account.amount),
                trade_direction,
                token_swap.fees(),
            )
            .ok_or(SwapError::ZeroTradingTokens)?;

        // Re-calculate the source amount swapped based on what the curve says
        //         重新计算的核心目的是：
        // 	1.	确保交易费用被正确计算并加到源代币或目标代币的金额中。
        // 	2.	根据当前周期、代币小数位和费用策略动态调整金额。
        // 	3.	防止滑点过大导致交易失败，通过计算实际接收金额并与最低接收金额进行比较，保护用户免受不合理的交易条件。
        // 	4.	解决源代币和目标代币数量不一致的情况，确保在交易后得出的金额符合预期。

        // 重新计算不仅是为了确保交易金额的准确性，还能保证交易的公平性、合理性和防止潜在的错误。
        // 源代币计算: 根据源代币的交换数量和费用配置，重新计算源代币的实际交换数量。
        let (source_transfer_amount, source_mint_decimals) = {
            let source_amount_swapped = to_u64(result.source_amount_swapped)?;

            let source_mint_data = source_token_mint_info.data.borrow();
            let source_mint = Self::unpack_mint_with_extensions(
                &source_mint_data,
                source_token_mint_info.owner,
                token_swap.token_program_id(),
            )?;
            // 调用 calculate_inverse_epoch_fee 来计算与当前周期相关的费用，并将其加到源代币交换数量 source_amount_swapped 上
            // •	源代币加法：计算转账费用时，源代币数量增加，因为用户支付的费用会加到源代币金额上，实际转账金额增加。
            let amount =
                if let Ok(transfer_fee_config) = source_mint.get_extension::<TransferFeeConfig>() {
                    source_amount_swapped.saturating_add(
                        transfer_fee_config
                            .calculate_inverse_epoch_fee(Clock::get()?.epoch, source_amount_swapped)
                            .ok_or(SwapError::FeeCalculationFailure)?,
                    )
                } else {
                    source_amount_swapped
                };
            (amount, source_mint.base.decimals)
        };
        // 目标代币计算: 根据目标代币的交换数量、费用配置以及滑点限制，重新计算目标代币的实际交换数量，并判断是否满足最低输出要求。
        // 目标代币减法：计算目标代币费用时，目标代币数量减少，因为用户实际收到的目标代币会扣除费用，最终数量减少。
        let (destination_transfer_amount, destination_mint_decimals) = {
            let destination_mint_data = destination_token_mint_info.data.borrow();
            let destination_mint = Self::unpack_mint_with_extensions(
                &destination_mint_data,
                source_token_mint_info.owner,
                token_swap.token_program_id(),
            )?;
            let amount_out = to_u64(result.destination_amount_swapped)?;
            // 尝试从目标代币的铸造数据中获取 TransferFeeConfig 扩展，计算目标代币的费用。通过调用 calculate_epoch_fee 计算当前周期的费用，并从目标代币的数量中减去。
            let amount_received = if let Ok(transfer_fee_config) =
                destination_mint.get_extension::<TransferFeeConfig>()
            {
                amount_out.saturating_sub(
                    transfer_fee_config
                        .calculate_epoch_fee(Clock::get()?.epoch, amount_out)
                        .ok_or(SwapError::FeeCalculationFailure)?,
                )
            } else {
                amount_out
            };
            // 计算 amount_received，如果低于 minimum_amount_out，则交易失败，避免滑点过大。
            if amount_received < minimum_amount_out {
                return Err(SwapError::ExceededSlippage.into());
            }
            (amount_out, destination_mint.base.decimals)
        };

        let (swap_token_a_amount, swap_token_b_amount) = match trade_direction {
            TradeDirection::AtoB => (
                result.new_swap_source_amount,
                result.new_swap_destination_amount,
            ),
            TradeDirection::BtoA => (
                result.new_swap_destination_amount,
                result.new_swap_source_amount,
            ),
        };
        // 用户 -> 交换池：转移 source_transfer_amount 代币
        Self::token_transfer(
            swap_info.key,
            source_token_program_info.clone(),
            source_info.clone(),
            source_token_mint_info.clone(),
            swap_source_info.clone(),
            user_transfer_authority_info.clone(),
            token_swap.bump_seed(),
            source_transfer_amount,
            source_mint_decimals,
        )?;
        // 计算协议费用，并可能分配给流动性提供者。
        if result.owner_fee > 0 {
            // 计算所有者手续费的 Pool Token 数量
            let mut pool_token_amount = token_swap
                .swap_curve()
                .calculator
                .withdraw_single_token_type_exact_out(
                    result.owner_fee,
                    swap_token_a_amount,
                    swap_token_b_amount,
                    u128::from(pool_mint.supply),
                    trade_direction,
                    RoundDirection::Floor,
                )
                .ok_or(SwapError::FeeCalculationFailure)?;
            // Allow error to fall through
            // 计算并分配 Host Fee
            if let Ok(host_fee_account_info) = next_account_info(account_info_iter) {
                let host_fee_account = Self::unpack_token_account(
                    host_fee_account_info,
                    token_swap.token_program_id(),
                )?;
                if *pool_mint_info.key != host_fee_account.mint {
                    return Err(SwapError::IncorrectPoolMint.into());
                }
                let host_fee = token_swap
                    .fees()
                    .host_fee(pool_token_amount)
                    .ok_or(SwapError::FeeCalculationFailure)?;
                // 减少 Owner Fee 并铸造 Host Fee
                if host_fee > 0 {
                    pool_token_amount = pool_token_amount
                        .checked_sub(host_fee)
                        .ok_or(SwapError::FeeCalculationFailure)?;
                    Self::token_mint_to(
                        swap_info.key,
                        pool_token_program_info.clone(),
                        pool_mint_info.clone(),
                        host_fee_account_info.clone(),
                        authority_info.clone(),
                        token_swap.bump_seed(),
                        to_u64(host_fee)?,
                    )?;
                }
            }
            // 计算并分配 Pool Fee
            if token_swap
                .check_pool_fee_info(pool_fee_account_info)
                .is_ok()
            {
                Self::token_mint_to(
                    swap_info.key,
                    pool_token_program_info.clone(),
                    pool_mint_info.clone(),
                    pool_fee_account_info.clone(),
                    authority_info.clone(),
                    token_swap.bump_seed(),
                    to_u64(pool_token_amount)?,
                )?;
            };
        }
        // 交换池 -> 用户：转移 destination_transfer_amount 代币
        Self::token_transfer(
            swap_info.key,
            destination_token_program_info.clone(),
            swap_destination_info.clone(),
            destination_token_mint_info.clone(),
            destination_info.clone(),
            authority_info.clone(),
            token_swap.bump_seed(),
            destination_transfer_amount,
            destination_mint_decimals,
        )?;

        Ok(())
    }

    /// Processes an [DepositAllTokenTypes](enum.Instruction.html).
    /// process_deposit_all_token_types 函数用于处理用户将两种不同类型的代币（代币 A 和代币 B）存入流动性池。
    /// 它计算存入的代币数量，检查滑点（slippage），进行代币转账，并铸造池代币（代表用户在流动性池中的份额）。
    // 参数说明：
    // •	program_id: 部署的程序的公钥。
    // •	pool_token_amount: 用户希望存入的池代币（LP 代币）数量。
    // •	maximum_token_a_amount: 用户愿意存入的最大代币 A 数量。
    // •	maximum_token_b_amount: 用户愿意存入的最大代币 B 数量。
    // •	accounts: 一个包含所需账户信息的数组。
    pub fn process_deposit_all_token_types(
        program_id: &Pubkey,
        pool_token_amount: u64,
        maximum_token_a_amount: u64,
        maximum_token_b_amount: u64,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        // •	swap_info: 存储交换合约信息。
        // •	authority_info: 存储授权信息（如拥有流动性池的账户）。
        // •	user_transfer_authority_info: 存储用户的转账授权账户。
        // •	source_a_info, source_b_info: 存储代币 A 和代币 B 的源账户信息。
        // •	token_a_info, token_b_info: 存储代币 A 和代币 B 的目标账户信息。
        // •	pool_mint_info: 存储池代币的 mint 信息。
        // •	dest_info: 存储目标账户的信息（池代币的接收方）。
        // •	其他几个账户信息涉及代币 mint 和程序的具体实现。
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let source_a_info = next_account_info(account_info_iter)?;
        let source_b_info = next_account_info(account_info_iter)?;
        let token_a_info = next_account_info(account_info_iter)?;
        let token_b_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let dest_info = next_account_info(account_info_iter)?;
        let token_a_mint_info = next_account_info(account_info_iter)?;
        let token_b_mint_info = next_account_info(account_info_iter)?;
        let token_a_program_info = next_account_info(account_info_iter)?;
        let token_b_program_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;

        // 解包交换信息和校验支持存款操作
        let token_swap = SwapVersion::unpack(&swap_info.data.borrow())?;
        let calculator = &token_swap.swap_curve().calculator;
        if !calculator.allows_deposits() {
            return Err(SwapError::UnsupportedCurveOperation.into());
        }

        // 账户信息验证
        Self::check_accounts(
            token_swap.as_ref(),
            program_id,
            swap_info,
            authority_info,
            token_a_info,
            token_b_info,
            pool_mint_info,
            pool_token_program_info,
            Some(source_a_info),
            Some(source_b_info),
            None,
        )?;

        // 解包代币账户和池代币信息
        let token_a = Self::unpack_token_account(token_a_info, token_swap.token_program_id())?;
        let token_b = Self::unpack_token_account(token_b_info, token_swap.token_program_id())?;
        let pool_mint = Self::unpack_mint(pool_mint_info, token_swap.token_program_id())?;
        let current_pool_mint_supply = u128::from(pool_mint.supply);
        // 计算新池代币供应量
        //     •	已有池：如果池代币已经存在（current_pool_mint_supply > 0），则使用用户希望存入的 pool_token_amount 作为新存入的池代币数量，并保持现有的池代币总供应量。
        //     •	新池：如果池代币尚不存在（current_pool_mint_supply <= 0），则为新池生成初始池代币数量和总供应量，通常通过计算器方法 calculator.new_pool_supply() 来决定这些值。

        // 这样设计的目的是为了在已有流动性池的情况下，按比例增加池代币供应量；而在新建池的情况下，生成一个合理的初始池代币供应量。
        let (pool_token_amount, pool_mint_supply) = if current_pool_mint_supply > 0 {
            (u128::from(pool_token_amount), current_pool_mint_supply)
        } else {
            (calculator.new_pool_supply(), calculator.new_pool_supply())
        };
        // 计算应得的代币数量
        let results = calculator
            .pool_tokens_to_trading_tokens(
                pool_token_amount,
                pool_mint_supply,
                u128::from(token_a.amount),
                u128::from(token_b.amount),
                RoundDirection::Ceiling,
            )
            .ok_or(SwapError::ZeroTradingTokens)?;
        let token_a_amount = to_u64(results.token_a_amount)?;
        // 滑点检查
        if token_a_amount > maximum_token_a_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if token_a_amount == 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }
        let token_b_amount = to_u64(results.token_b_amount)?;
        if token_b_amount > maximum_token_b_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if token_b_amount == 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }

        let pool_token_amount = to_u64(pool_token_amount)?;
        // 执行代币转账和池代币铸造
        Self::token_transfer(
            swap_info.key,
            token_a_program_info.clone(),
            source_a_info.clone(),
            token_a_mint_info.clone(),
            token_a_info.clone(),
            user_transfer_authority_info.clone(),
            token_swap.bump_seed(),
            token_a_amount,
            Self::unpack_mint(token_a_mint_info, token_swap.token_program_id())?.decimals,
        )?;
        Self::token_transfer(
            swap_info.key,
            token_b_program_info.clone(),
            source_b_info.clone(),
            token_b_mint_info.clone(),
            token_b_info.clone(),
            user_transfer_authority_info.clone(),
            token_swap.bump_seed(),
            token_b_amount,
            Self::unpack_mint(token_b_mint_info, token_swap.token_program_id())?.decimals,
        )?;
        // 使用 Self::token_mint_to 铸造池代币，并将其发送到目标账户。
        Self::token_mint_to(
            swap_info.key,
            pool_token_program_info.clone(),
            pool_mint_info.clone(),
            dest_info.clone(),
            authority_info.clone(),
            token_swap.bump_seed(),
            pool_token_amount,
        )?;

        Ok(())
    }

    /// Processes an [WithdrawAllTokenTypes](enum.Instruction.html).
    /// 	•	该函数的目标是处理用户通过池代币提取交易池中代币 A 和代币 B 的操作。
	// •	在提现过程中，考虑了提现费用、池代币的销毁、代币的转移以及最小金额限制等多个因素。
	// •	通过 check_accounts 方法验证所有账户的合法性，确保操作的正确性。
	// •	涉及了池代币、交易代币之间的复杂计算，特别是如何根据池代币数量计算对应的交易代币数量。
    pub fn process_withdraw_all_token_types(
        program_id: &Pubkey,
        pool_token_amount: u64,
        minimum_token_a_amount: u64,
        minimum_token_b_amount: u64,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        // 初始化账户信息
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let source_info = next_account_info(account_info_iter)?;
        let token_a_info = next_account_info(account_info_iter)?;
        let token_b_info = next_account_info(account_info_iter)?;
        let dest_token_a_info = next_account_info(account_info_iter)?;
        let dest_token_b_info = next_account_info(account_info_iter)?;
        let pool_fee_account_info = next_account_info(account_info_iter)?;
        let token_a_mint_info = next_account_info(account_info_iter)?;
        let token_b_mint_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;
        let token_a_program_info = next_account_info(account_info_iter)?;
        let token_b_program_info = next_account_info(account_info_iter)?;

        let token_swap = SwapVersion::unpack(&swap_info.data.borrow())?;
        // 检查账户的合法性
        Self::check_accounts(
            token_swap.as_ref(),
            program_id,
            swap_info,
            authority_info,
            token_a_info,
            token_b_info,
            pool_mint_info,
            pool_token_program_info,
            Some(dest_token_a_info),
            Some(dest_token_b_info),
            Some(pool_fee_account_info),
        )?;

        let token_a = Self::unpack_token_account(token_a_info, token_swap.token_program_id())?;
        let token_b = Self::unpack_token_account(token_b_info, token_swap.token_program_id())?;
        let pool_mint = Self::unpack_mint(pool_mint_info, token_swap.token_program_id())?;

        let calculator = &token_swap.swap_curve().calculator;
        // 计算提现费
        let withdraw_fee = match token_swap.check_pool_fee_info(pool_fee_account_info) {
            Ok(_) => {
                if *pool_fee_account_info.key == *source_info.key {
                    // withdrawing from the fee account, don't assess withdraw fee
                    0
                } else {
                    token_swap
                        .fees()
                        .owner_withdraw_fee(u128::from(pool_token_amount))
                        .ok_or(SwapError::FeeCalculationFailure)?
                }
            }
            Err(_) => 0,
        };
        // 根据计算出的提现费用调整用户请求提现的池代币数量，确保提现费用已经从池代币数量中扣除。
        let pool_token_amount = u128::from(pool_token_amount)
            .checked_sub(withdraw_fee)
            .ok_or(SwapError::CalculationFailure)?;
        // 使用池代币数量、池代币供应量以及当前池内代币 A 和代币 B 的数量，利用交换曲线（calculator）来计算应该提现的代币 A 和代币 B 的数量。
        let results = calculator
            .pool_tokens_to_trading_tokens(
                pool_token_amount,
                u128::from(pool_mint.supply),
                u128::from(token_a.amount),
                u128::from(token_b.amount),
                RoundDirection::Floor,
            )
            .ok_or(SwapError::ZeroTradingTokens)?;

        // 通过 to_u64 将计算结果转换为 u64，并确保计算的提现数量不小于用户设置的最小值（minimum_token_a_amount 和 minimum_token_b_amount）。
        // 如果满足条件，继续执行，否则返回错误。

        let token_a_amount = to_u64(results.token_a_amount)?;
        let token_a_amount = std::cmp::min(token_a.amount, token_a_amount);
        if token_a_amount < minimum_token_a_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if token_a_amount == 0 && token_a.amount != 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }
        let token_b_amount = to_u64(results.token_b_amount)?;
        let token_b_amount = std::cmp::min(token_b.amount, token_b_amount);
        if token_b_amount < minimum_token_b_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if token_b_amount == 0 && token_b.amount != 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }
        // 如果提现费用大于 0，则将提现费用从用户账户转移到费用账户。
        if withdraw_fee > 0 {
            Self::token_transfer(
                swap_info.key,
                pool_token_program_info.clone(),
                source_info.clone(),
                pool_mint_info.clone(),
                pool_fee_account_info.clone(),
                user_transfer_authority_info.clone(),
                token_swap.bump_seed(),
                to_u64(withdraw_fee)?,
                pool_mint.decimals,
            )?;
        }
        // 销毁池代币，即从用户账户中扣除相应数量的池代币。
        Self::token_burn(
            swap_info.key,
            pool_token_program_info.clone(),
            source_info.clone(),
            pool_mint_info.clone(),
            user_transfer_authority_info.clone(),
            token_swap.bump_seed(),
            to_u64(pool_token_amount)?,
        )?;
        // 如果有代币 A 和代币 B 需要提取，则将其从池中转移到目标账户。
        if token_a_amount > 0 {
            Self::token_transfer(
                swap_info.key,
                token_a_program_info.clone(),
                token_a_info.clone(),
                token_a_mint_info.clone(),
                dest_token_a_info.clone(),
                authority_info.clone(),
                token_swap.bump_seed(),
                token_a_amount,
                Self::unpack_mint(token_a_mint_info, token_swap.token_program_id())?.decimals,
            )?;
        }
        if token_b_amount > 0 {
            Self::token_transfer(
                swap_info.key,
                token_b_program_info.clone(),
                token_b_info.clone(),
                token_b_mint_info.clone(),
                dest_token_b_info.clone(),
                authority_info.clone(),
                token_swap.bump_seed(),
                token_b_amount,
                Self::unpack_mint(token_b_mint_info, token_swap.token_program_id())?.decimals,
            )?;
        }
        Ok(())
    }

    /// Processes DepositSingleTokenTypeExactAmountIn
    /// 代币存入操作，用户存入一定数量的源代币后，系统根据当前的交换曲线计算出应该获得的池子代币数量，确保操作在规定的滑点范围内，然后执行代币转账和池子代币铸造的操作，最终完成存款过程。
    /// 	•	program_id: &Pubkey：调用此函数的智能合约程序的 ID。
	// •	source_token_amount: u64：用户存入的源代币数量。
	// •	minimum_pool_token_amount: u64：用户期望最低获得的池子代币数量，用于防止滑点过大。
	// •	accounts: &[AccountInfo]：一组账户信息，这些账户用于进行存款和代币转账操作。
    pub fn process_deposit_single_token_type_exact_amount_in(
        program_id: &Pubkey,
        source_token_amount: u64,
        minimum_pool_token_amount: u64,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        // 解析账户信息
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let source_info = next_account_info(account_info_iter)?;
        let swap_token_a_info = next_account_info(account_info_iter)?;
        let swap_token_b_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let destination_info = next_account_info(account_info_iter)?;
        let source_token_mint_info = next_account_info(account_info_iter)?;
        let source_token_program_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;

        // 从 swap_info 中解包出 token_swap 对象，它包含了交换协议的状态。然后获取 swap_curve（交换曲线），通过 calculator 来检查是否允许存款操作。如果不允许存款，函数会返回错误。
        let token_swap = SwapVersion::unpack(&swap_info.data.borrow())?;
        let calculator = &token_swap.swap_curve().calculator;
        if !calculator.allows_deposits() {
            return Err(SwapError::UnsupportedCurveOperation.into());
        }
        // 解包用户存入代币的账户，确保其有效性
        let source_account =
            Self::unpack_token_account(source_info, token_swap.token_program_id())?;
        let swap_token_a =
            Self::unpack_token_account(swap_token_a_info, token_swap.token_program_id())?;
        let swap_token_b =
            Self::unpack_token_account(swap_token_b_info, token_swap.token_program_id())?;

        // 确认交换方向
        let trade_direction = if source_account.mint == swap_token_a.mint {
            TradeDirection::AtoB
        } else if source_account.mint == swap_token_b.mint {
            TradeDirection::BtoA
        } else {
            return Err(SwapError::IncorrectSwapAccount.into());
        };
        
        let (source_a_info, source_b_info) = match trade_direction {
            TradeDirection::AtoB => (Some(source_info), None),
            TradeDirection::BtoA => (None, Some(source_info)),
        };
        // 账户验证
        Self::check_accounts(
            token_swap.as_ref(),
            program_id,
            swap_info,
            authority_info,
            swap_token_a_info,
            swap_token_b_info,
            pool_mint_info,
            pool_token_program_info,
            source_a_info,
            source_b_info,
            None,
        )?;

        let pool_mint = Self::unpack_mint(pool_mint_info, token_swap.token_program_id())?;
        let pool_mint_supply = u128::from(pool_mint.supply);
        // 池子代币的计算
        let pool_token_amount = if pool_mint_supply > 0 {
            token_swap
                .swap_curve()
                .deposit_single_token_type(
                    u128::from(source_token_amount),
                    u128::from(swap_token_a.amount),
                    u128::from(swap_token_b.amount),
                    pool_mint_supply,
                    trade_direction,
                    token_swap.fees(),
                )
                .ok_or(SwapError::ZeroTradingTokens)?
        } else {
            calculator.new_pool_supply()
        };
        
        let pool_token_amount = to_u64(pool_token_amount)?;
        // 如果计算出的池子代币数量小于 minimum_pool_token_amount，或者为 0，则返回错误，表示滑点过大或没有交易代币。
        if pool_token_amount < minimum_pool_token_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if pool_token_amount == 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }
        // 根据交易方向，将源代币转入相应的池子代币账户
        match trade_direction {
            TradeDirection::AtoB => {
                Self::token_transfer(
                    swap_info.key,
                    source_token_program_info.clone(),
                    source_info.clone(),
                    source_token_mint_info.clone(),
                    swap_token_a_info.clone(),
                    user_transfer_authority_info.clone(),
                    token_swap.bump_seed(),
                    source_token_amount,
                    Self::unpack_mint(source_token_mint_info, token_swap.token_program_id())?
                        .decimals,
                )?;
            }
            TradeDirection::BtoA => {
                Self::token_transfer(
                    swap_info.key,
                    source_token_program_info.clone(),
                    source_info.clone(),
                    source_token_mint_info.clone(),
                    swap_token_b_info.clone(),
                    user_transfer_authority_info.clone(),
                    token_swap.bump_seed(),
                    source_token_amount,
                    Self::unpack_mint(source_token_mint_info, token_swap.token_program_id())?
                        .decimals,
                )?;
            }
        }
        // 将计算出的池子代币数量铸造到用户的目标账户中
        Self::token_mint_to(
            swap_info.key,
            pool_token_program_info.clone(),
            pool_mint_info.clone(),
            destination_info.clone(),
            authority_info.clone(),
            token_swap.bump_seed(),
            pool_token_amount,
        )?;

        Ok(())
    }

    /// Processes a
    /// [WithdrawSingleTokenTypeExactAmountOut](enum.Instruction.html).
    /// 处理从去中心化交易池中提取单一代币，并确保提取的代币数量符合要求，同时考虑到手续费、池代币销毁等操作。
    /// 它确保了提取过程的安全性和正确性，通过一系列的计算、验证和代币操作，完成提现任务；
    ///•	program_id: 表示当前程序的公钥。
	// •	destination_token_amount: 这是用户希望提取的目标代币数量。
	// •	maximum_pool_token_amount: 用户愿意支付的最大池代币数量。
	// •	accounts: 包含所有与该操作相关的账户信息列表，包括池代币账户、授权账户等。
    pub fn process_withdraw_single_token_type_exact_amount_out(
        program_id: &Pubkey,
        destination_token_amount: u64,
        maximum_pool_token_amount: u64,
        accounts: &[AccountInfo],
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let swap_info = next_account_info(account_info_iter)?;
        let authority_info = next_account_info(account_info_iter)?;
        let user_transfer_authority_info = next_account_info(account_info_iter)?;
        let pool_mint_info = next_account_info(account_info_iter)?;
        let source_info = next_account_info(account_info_iter)?;
        let swap_token_a_info = next_account_info(account_info_iter)?;
        let swap_token_b_info = next_account_info(account_info_iter)?;
        let destination_info = next_account_info(account_info_iter)?;
        let pool_fee_account_info = next_account_info(account_info_iter)?;
        let destination_token_mint_info = next_account_info(account_info_iter)?;
        let pool_token_program_info = next_account_info(account_info_iter)?;
        let destination_token_program_info = next_account_info(account_info_iter)?;

        let token_swap = SwapVersion::unpack(&swap_info.data.borrow())?;
        let destination_account =
            Self::unpack_token_account(destination_info, token_swap.token_program_id())?;
        let swap_token_a =
            Self::unpack_token_account(swap_token_a_info, token_swap.token_program_id())?;
        let swap_token_b =
            Self::unpack_token_account(swap_token_b_info, token_swap.token_program_id())?;

        let trade_direction = if destination_account.mint == swap_token_a.mint {
            TradeDirection::AtoB
        } else if destination_account.mint == swap_token_b.mint {
            TradeDirection::BtoA
        } else {
            return Err(SwapError::IncorrectSwapAccount.into());
        };

        let (destination_a_info, destination_b_info) = match trade_direction {
            TradeDirection::AtoB => (Some(destination_info), None),
            TradeDirection::BtoA => (None, Some(destination_info)),
        };
        Self::check_accounts(
            token_swap.as_ref(),
            program_id,
            swap_info,
            authority_info,
            swap_token_a_info,
            swap_token_b_info,
            pool_mint_info,
            pool_token_program_info,
            destination_a_info,
            destination_b_info,
            Some(pool_fee_account_info),
        )?;

        let pool_mint = Self::unpack_mint(pool_mint_info, token_swap.token_program_id())?;
        let pool_mint_supply = u128::from(pool_mint.supply);
        let swap_token_a_amount = u128::from(swap_token_a.amount);
        let swap_token_b_amount = u128::from(swap_token_b.amount);
        // 计算用户提取指定数量的目标代币时需要销毁的池代币数量。这个计算会根据当前的池代币数量、目标代币数量、交易方向等因素来确定。
        let burn_pool_token_amount = token_swap
            .swap_curve()
            .withdraw_single_token_type_exact_out(
                u128::from(destination_token_amount),
                swap_token_a_amount,
                swap_token_b_amount,
                pool_mint_supply,
                trade_direction,
                token_swap.fees(),
            )
            .ok_or(SwapError::ZeroTradingTokens)?;
        // 计算提现费用，如果提现是从池费用账户中提取的，就不收取手续费。否则根据交换池的规则收取一定的手续费。
        let withdraw_fee = match token_swap.check_pool_fee_info(pool_fee_account_info) {
            Ok(_) => {
                if *pool_fee_account_info.key == *source_info.key {
                    // withdrawing from the fee account, don't assess withdraw fee
                    0
                } else {
                    token_swap
                        .fees()
                        .owner_withdraw_fee(burn_pool_token_amount)
                        .ok_or(SwapError::FeeCalculationFailure)?
                }
            }
            Err(_) => 0,
        };
        // 确保计算出来的池代币数量没有超过用户设定的最大值，避免因滑点导致不合理的提现数量
        let pool_token_amount = burn_pool_token_amount
            .checked_add(withdraw_fee)
            .ok_or(SwapError::CalculationFailure)?;

        if to_u64(pool_token_amount)? > maximum_pool_token_amount {
            return Err(SwapError::ExceededSlippage.into());
        }
        if pool_token_amount == 0 {
            return Err(SwapError::ZeroTradingTokens.into());
        }
        // 手续费转移和池代币销毁
        if withdraw_fee > 0 {
            Self::token_transfer(
                swap_info.key,
                pool_token_program_info.clone(),
                source_info.clone(),
                pool_mint_info.clone(),
                pool_fee_account_info.clone(),
                user_transfer_authority_info.clone(),
                token_swap.bump_seed(),
                to_u64(withdraw_fee)?,
                pool_mint.decimals,
            )?;
        }
        Self::token_burn(
            swap_info.key,
            pool_token_program_info.clone(),
            source_info.clone(),
            pool_mint_info.clone(),
            user_transfer_authority_info.clone(),
            token_swap.bump_seed(),
            to_u64(burn_pool_token_amount)?,
        )?;
        // 根据交易方向，将目标代币（swap_token_a 或 swap_token_b）转移到目标账户中
        match trade_direction {
            TradeDirection::AtoB => {
                Self::token_transfer(
                    swap_info.key,
                    destination_token_program_info.clone(),
                    swap_token_a_info.clone(),
                    destination_token_mint_info.clone(),
                    destination_info.clone(),
                    authority_info.clone(),
                    token_swap.bump_seed(),
                    destination_token_amount,
                    Self::unpack_mint(destination_token_mint_info, token_swap.token_program_id())?
                        .decimals,
                )?;
            }
            TradeDirection::BtoA => {
                Self::token_transfer(
                    swap_info.key,
                    destination_token_program_info.clone(),
                    swap_token_b_info.clone(),
                    destination_token_mint_info.clone(),
                    destination_info.clone(),
                    authority_info.clone(),
                    token_swap.bump_seed(),
                    destination_token_amount,
                    Self::unpack_mint(destination_token_mint_info, token_swap.token_program_id())?
                        .decimals,
                )?;
            }
        }

        Ok(())
    }

    /// Processes an [Instruction](enum.Instruction.html).  处理所有swap相关的指令
    pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], input: &[u8]) -> ProgramResult {
        Self::process_with_constraints(program_id, accounts, input, &SWAP_CONSTRAINTS)
    }

    /// Processes an instruction given extra constraint
    /// process_with_constraints 方法是 Solana Token Swap 程序 的 指令（instruction）处理器，用于解析和执行不同类型的流动性池操作。

    // 这个方法的作用是：
    // 1.	解析输入数据，将其转换为 SwapInstruction 枚举类型的具体指令。
    // 2.	匹配不同的指令类型，并调用相应的处理函数（如 process_initialize、process_swap 等）。
    // 3.	执行流动性池相关操作，如 初始化池子、交换代币、存取流动性等，并在执行过程中检查是否需要额外的 约束（swap_constraints）。

    // •	program_id：当前合约程序的 Pubkey，用于校验该交易属于 Token Swap 程序。
    // •	accounts：涉及的 Solana 账户，如流动性池账户、用户账户等。
    // •	input：指令的二进制数据，需要解包（deserialize）成 SwapInstruction 以确定要执行的操作。
    // •	swap_constraints：额外的约束条件（可选），可能用于 限制某些交易行为，比如 最大/最小流动性存取额度、交易滑点等。
    // •	返回值：ProgramResult，表示执行结果。如果执行成功，返回 Ok(())，否则返回 Err(SwapError::XXX)。
    pub fn process_with_constraints(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        input: &[u8],
        swap_constraints: &Option<SwapConstraints>,
    ) -> ProgramResult {
        let instruction = SwapInstruction::unpack(input)?;
        match instruction {
            //初始化
            //1. 初始化流动性池
            //         解析 Initialize 指令，包含：
            // •	fees：池子的手续费设定。
            // •	swap_curve：池子使用的 AMM 交易曲线类型（如 ConstantProduct、ConstantPrice）。
            // •	调用 process_initialize 处理池子创建逻辑。
            SwapInstruction::Initialize(Initialize { fees, swap_curve }) => {
                msg!("Instruction: Init");
                Self::process_initialize(program_id, fees, swap_curve, accounts, swap_constraints)
            }
            // 2. 代币交换（Swap）
            // •	执行代币交换，将 TokenA -> TokenB 或 TokenB -> TokenA。
            // •	amount_in：用户提供的输入代币数量。
            // •	minimum_amount_out：用户期望获得的最小输出代币数量（用于防止滑点过大）。
            // •	由 process_swap 处理实际的兑换逻辑。
            SwapInstruction::Swap(Swap {
                amount_in,
                minimum_amount_out,
            }) => {
                msg!("Instruction: Swap");
                Self::process_swap(program_id, amount_in, minimum_amount_out, accounts)
            }
            // 3. 双边存入流动性（DepositAllTokenTypes）
            // •	向流动性池存入 TokenA 和 TokenB，获取流动性代币（LP Token）。
            // •	pool_token_amount：希望获得的 LP 代币数量。
            // •	maximum_token_a_amount / maximum_token_b_amount：存入的最大 Token A / B 数量（超出部分不存入）。
            // •	process_deposit_all_token_types 计算需要存入的 TokenA/B，并处理流动性提供逻辑。
            SwapInstruction::DepositAllTokenTypes(DepositAllTokenTypes {
                pool_token_amount,
                maximum_token_a_amount,
                maximum_token_b_amount,
            }) => {
                msg!("Instruction: DepositAllTokenTypes");
                Self::process_deposit_all_token_types(
                    program_id,
                    pool_token_amount,
                    maximum_token_a_amount,
                    maximum_token_b_amount,
                    accounts,
                )
            }
            // 4. 双边取出流动性（WithdrawAllTokenTypes）
            // •	从流动性池提取 TokenA 和 TokenB，销毁 LP 代币。
            // •	pool_token_amount：要销毁的 LP 代币数量。
            // •	minimum_token_a_amount / minimum_token_b_amount：用户希望至少收到的 Token A / B 数量（防止滑点损失）。
            // •	由 process_withdraw_all_token_types 计算实际可提取的 TokenA/B，并执行提款操作。
            SwapInstruction::WithdrawAllTokenTypes(WithdrawAllTokenTypes {
                pool_token_amount,
                minimum_token_a_amount,
                minimum_token_b_amount,
            }) => {
                msg!("Instruction: WithdrawAllTokenTypes");
                Self::process_withdraw_all_token_types(
                    program_id,
                    pool_token_amount,
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                    accounts,
                )
            }
            // 5. 单边存款（DepositSingleTokenTypeExactAmountIn）
            // •	只存入 TokenA 或 TokenB，获取 LP 代币（单边存入）。
            // •	source_token_amount：存入的 TokenA 或 TokenB 数量。
            // •	minimum_pool_token_amount：至少希望获得的 LP 代币数量（防止滑点影响）。
            // •	由 process_deposit_single_token_type_exact_amount_in 处理实际计算。
            SwapInstruction::DepositSingleTokenTypeExactAmountIn(
                DepositSingleTokenTypeExactAmountIn {
                    source_token_amount,
                    minimum_pool_token_amount,
                },
            ) => {
                msg!("Instruction: DepositSingleTokenTypeExactAmountIn");
                Self::process_deposit_single_token_type_exact_amount_in(
                    program_id,
                    source_token_amount,
                    minimum_pool_token_amount,
                    accounts,
                )
            }
            // 6. 单边取款（WithdrawSingleTokenTypeExactAmountOut）
            // •	只提取 TokenA 或 TokenB，销毁 LP 代币（单边提取）。
            // •	destination_token_amount：用户希望取出的 TokenA 或 TokenB 数量。
            // •	maximum_pool_token_amount：用户最多愿意销毁的 LP 代币数量（防止滑点过高）。
            // •	由 process_withdraw_single_token_type_exact_amount_out 处理计算与提款逻辑。
            SwapInstruction::WithdrawSingleTokenTypeExactAmountOut(
                WithdrawSingleTokenTypeExactAmountOut {
                    destination_token_amount,
                    maximum_pool_token_amount,
                },
            ) => {
                msg!("Instruction: WithdrawSingleTokenTypeExactAmountOut");
                Self::process_withdraw_single_token_type_exact_amount_out(
                    program_id,
                    destination_token_amount,
                    maximum_pool_token_amount,
                    accounts,
                )
            }
        }
    }
}

fn to_u64(val: u128) -> Result<u64, SwapError> {
    val.try_into().map_err(|_| SwapError::ConversionFailure)
}

fn invoke_signed_wrapper<T>(
    instruction: &Instruction,
    account_infos: &[AccountInfo],
    signers_seeds: &[&[&[u8]]],
) -> Result<(), ProgramError>
where
    T: 'static + PrintProgramError + DecodeError<T> + FromPrimitive + Error,
{
    invoke_signed(instruction, account_infos, signers_seeds).inspect_err(|err| {
        err.print::<T>();
    })
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::{
            curve::{
                base::CurveType,
                calculator::{CurveCalculator, INITIAL_SWAP_POOL_AMOUNT},
                constant_price::ConstantPriceCurve,
                constant_product::ConstantProductCurve,
                offset::OffsetCurve,
            },
            instruction::{
                deposit_all_token_types, deposit_single_token_type_exact_amount_in, initialize,
                swap, withdraw_all_token_types, withdraw_single_token_type_exact_amount_out,
            },
        },
        solana_program::{
            clock::Clock, entrypoint::SUCCESS, instruction::Instruction, program_pack::Pack,
            program_stubs, rent::Rent,
        },
        solana_sdk::account::{
            create_account_for_test, create_is_signer_account_infos, Account as SolanaAccount,
        },
        spl_token_2022::{
            error::TokenError,
            extension::{
                transfer_fee::{instruction::initialize_transfer_fee_config, TransferFee},
                ExtensionType,
            },
            instruction::{
                approve, close_account, freeze_account, initialize_account,
                initialize_immutable_owner, initialize_mint, initialize_mint_close_authority,
                mint_to, revoke, set_authority, AuthorityType,
            },
        },
        std::sync::Arc,
        test_case::test_case,
    };

    // Test program id for the swap program.
    const SWAP_PROGRAM_ID: Pubkey = Pubkey::new_from_array([2u8; 32]);

    struct TestSyscallStubs {}
    impl program_stubs::SyscallStubs for TestSyscallStubs {
        fn sol_invoke_signed(
            &self,
            instruction: &Instruction,
            account_infos: &[AccountInfo],
            signers_seeds: &[&[&[u8]]],
        ) -> ProgramResult {
            msg!("TestSyscallStubs::sol_invoke_signed()");

            let mut new_account_infos = vec![];

            // mimic check for token program in accounts
            if !account_infos
                .iter()
                .any(|x| *x.key == spl_token::id() || *x.key == spl_token_2022::id())
            {
                return Err(ProgramError::InvalidAccountData);
            }

            for meta in instruction.accounts.iter() {
                for account_info in account_infos.iter() {
                    if meta.pubkey == *account_info.key {
                        let mut new_account_info = account_info.clone();
                        for seeds in signers_seeds.iter() {
                            let signer =
                                Pubkey::create_program_address(seeds, &SWAP_PROGRAM_ID).unwrap();
                            if *account_info.key == signer {
                                new_account_info.is_signer = true;
                            }
                        }
                        new_account_infos.push(new_account_info);
                    }
                }
            }

            if instruction.program_id == spl_token::id() {
                spl_token::processor::Processor::process(
                    &instruction.program_id,
                    &new_account_infos,
                    &instruction.data,
                )
            } else if instruction.program_id == spl_token_2022::id() {
                spl_token_2022::processor::Processor::process(
                    &instruction.program_id,
                    &new_account_infos,
                    &instruction.data,
                )
            } else {
                Err(ProgramError::IncorrectProgramId)
            }
        }

        fn sol_get_clock_sysvar(&self, var_addr: *mut u8) -> u64 {
            unsafe {
                *(var_addr as *mut _ as *mut Clock) = Clock::default();
            }
            SUCCESS
        }
    }

    fn test_syscall_stubs() {
        use std::sync::Once;
        static ONCE: Once = Once::new();

        ONCE.call_once(|| {
            program_stubs::set_syscall_stubs(Box::new(TestSyscallStubs {}));
        });
    }

    #[derive(Default)]
    struct SwapTransferFees {
        pool_token: TransferFee,
        token_a: TransferFee,
        token_b: TransferFee,
    }

    struct SwapAccountInfo {
        bump_seed: u8,
        authority_key: Pubkey,
        fees: Fees,
        transfer_fees: SwapTransferFees,
        swap_curve: SwapCurve,
        swap_key: Pubkey,
        swap_account: SolanaAccount,
        pool_mint_key: Pubkey,
        pool_mint_account: SolanaAccount,
        pool_fee_key: Pubkey,
        pool_fee_account: SolanaAccount,
        pool_token_key: Pubkey,
        pool_token_account: SolanaAccount,
        token_a_key: Pubkey,
        token_a_account: SolanaAccount,
        token_a_mint_key: Pubkey,
        token_a_mint_account: SolanaAccount,
        token_b_key: Pubkey,
        token_b_account: SolanaAccount,
        token_b_mint_key: Pubkey,
        token_b_mint_account: SolanaAccount,
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    }

    impl SwapAccountInfo {
        #[allow(clippy::too_many_arguments)]
        pub fn new(
            user_key: &Pubkey,
            fees: Fees,
            transfer_fees: SwapTransferFees,
            swap_curve: SwapCurve,
            token_a_amount: u64,
            token_b_amount: u64,
            pool_token_program_id: &Pubkey,
            token_a_program_id: &Pubkey,
            token_b_program_id: &Pubkey,
        ) -> Self {
            let swap_key = Pubkey::new_unique();
            let swap_account = SolanaAccount::new(0, SwapVersion::LATEST_LEN, &SWAP_PROGRAM_ID);
            let (authority_key, bump_seed) =
                Pubkey::find_program_address(&[&swap_key.to_bytes()[..]], &SWAP_PROGRAM_ID);

            let (pool_mint_key, mut pool_mint_account) = create_mint(
                pool_token_program_id,
                &authority_key,
                None,
                None,
                &transfer_fees.pool_token,
            );
            let (pool_token_key, pool_token_account) = mint_token(
                pool_token_program_id,
                &pool_mint_key,
                &mut pool_mint_account,
                &authority_key,
                user_key,
                0,
            );
            let (pool_fee_key, pool_fee_account) = mint_token(
                pool_token_program_id,
                &pool_mint_key,
                &mut pool_mint_account,
                &authority_key,
                user_key,
                0,
            );
            let (token_a_mint_key, mut token_a_mint_account) = create_mint(
                token_a_program_id,
                user_key,
                None,
                None,
                &transfer_fees.token_a,
            );
            let (token_a_key, token_a_account) = mint_token(
                token_a_program_id,
                &token_a_mint_key,
                &mut token_a_mint_account,
                user_key,
                &authority_key,
                token_a_amount,
            );
            let (token_b_mint_key, mut token_b_mint_account) = create_mint(
                token_b_program_id,
                user_key,
                None,
                None,
                &transfer_fees.token_b,
            );
            let (token_b_key, token_b_account) = mint_token(
                token_b_program_id,
                &token_b_mint_key,
                &mut token_b_mint_account,
                user_key,
                &authority_key,
                token_b_amount,
            );

            SwapAccountInfo {
                bump_seed,
                authority_key,
                fees,
                transfer_fees,
                swap_curve,
                swap_key,
                swap_account,
                pool_mint_key,
                pool_mint_account,
                pool_fee_key,
                pool_fee_account,
                pool_token_key,
                pool_token_account,
                token_a_key,
                token_a_account,
                token_a_mint_key,
                token_a_mint_account,
                token_b_key,
                token_b_account,
                token_b_mint_key,
                token_b_mint_account,
                pool_token_program_id: *pool_token_program_id,
                token_a_program_id: *token_a_program_id,
                token_b_program_id: *token_b_program_id,
            }
        }

        pub fn initialize_swap(&mut self) -> ProgramResult {
            do_process_instruction(
                initialize(
                    &SWAP_PROGRAM_ID,
                    &self.pool_token_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &self.token_a_key,
                    &self.token_b_key,
                    &self.pool_mint_key,
                    &self.pool_fee_key,
                    &self.pool_token_key,
                    self.fees.clone(),
                    self.swap_curve.clone(),
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut self.token_a_account,
                    &mut self.token_b_account,
                    &mut self.pool_mint_account,
                    &mut self.pool_fee_account,
                    &mut self.pool_token_account,
                    &mut SolanaAccount::default(),
                ],
            )
        }

        pub fn setup_token_accounts(
            &mut self,
            mint_owner: &Pubkey,
            account_owner: &Pubkey,
            a_amount: u64,
            b_amount: u64,
            pool_amount: u64,
        ) -> (
            Pubkey,
            SolanaAccount,
            Pubkey,
            SolanaAccount,
            Pubkey,
            SolanaAccount,
        ) {
            let (token_a_key, token_a_account) = mint_token(
                &self.token_a_program_id,
                &self.token_a_mint_key,
                &mut self.token_a_mint_account,
                mint_owner,
                account_owner,
                a_amount,
            );
            let (token_b_key, token_b_account) = mint_token(
                &self.token_b_program_id,
                &self.token_b_mint_key,
                &mut self.token_b_mint_account,
                mint_owner,
                account_owner,
                b_amount,
            );
            let (pool_key, pool_account) = mint_token(
                &self.pool_token_program_id,
                &self.pool_mint_key,
                &mut self.pool_mint_account,
                &self.authority_key,
                account_owner,
                pool_amount,
            );
            (
                token_a_key,
                token_a_account,
                token_b_key,
                token_b_account,
                pool_key,
                pool_account,
            )
        }

        fn get_swap_key(&self, mint_key: &Pubkey) -> &Pubkey {
            if *mint_key == self.token_a_mint_key {
                &self.token_a_key
            } else if *mint_key == self.token_b_mint_key {
                &self.token_b_key
            } else {
                panic!("Could not find matching swap token account");
            }
        }

        fn get_token_program_id(&self, account_key: &Pubkey) -> &Pubkey {
            if *account_key == self.token_a_key {
                &self.token_a_program_id
            } else if *account_key == self.token_b_key {
                &self.token_b_program_id
            } else {
                panic!("Could not find matching swap token account");
            }
        }

        fn get_token_mint(&self, account_key: &Pubkey) -> (Pubkey, SolanaAccount) {
            if *account_key == self.token_a_key {
                (self.token_a_mint_key, self.token_a_mint_account.clone())
            } else if *account_key == self.token_b_key {
                (self.token_b_mint_key, self.token_b_mint_account.clone())
            } else {
                panic!("Could not find matching swap token account");
            }
        }

        fn get_token_account(&self, account_key: &Pubkey) -> &SolanaAccount {
            if *account_key == self.token_a_key {
                &self.token_a_account
            } else if *account_key == self.token_b_key {
                &self.token_b_account
            } else {
                panic!("Could not find matching swap token account");
            }
        }

        fn set_token_account(&mut self, account_key: &Pubkey, account: SolanaAccount) {
            if *account_key == self.token_a_key {
                self.token_a_account = account;
                return;
            } else if *account_key == self.token_b_key {
                self.token_b_account = account;
                return;
            }
            panic!("Could not find matching swap token account");
        }

        #[allow(clippy::too_many_arguments)]
        pub fn swap(
            &mut self,
            user_key: &Pubkey,
            user_source_key: &Pubkey,
            user_source_account: &mut SolanaAccount,
            swap_source_key: &Pubkey,
            swap_destination_key: &Pubkey,
            user_destination_key: &Pubkey,
            user_destination_account: &mut SolanaAccount,
            amount_in: u64,
            minimum_amount_out: u64,
        ) -> ProgramResult {
            let user_transfer_key = Pubkey::new_unique();
            let source_token_program_id = self.get_token_program_id(swap_source_key);
            let destination_token_program_id = self.get_token_program_id(swap_destination_key);
            // approve moving from user source account
            do_process_instruction(
                approve(
                    source_token_program_id,
                    user_source_key,
                    &user_transfer_key,
                    user_key,
                    &[],
                    amount_in,
                )
                .unwrap(),
                vec![
                    user_source_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            let (source_mint_key, mut source_mint_account) = self.get_token_mint(swap_source_key);
            let (destination_mint_key, mut destination_mint_account) =
                self.get_token_mint(swap_destination_key);
            let mut swap_source_account = self.get_token_account(swap_source_key).clone();
            let mut swap_destination_account = self.get_token_account(swap_destination_key).clone();

            // perform the swap
            do_process_instruction(
                swap(
                    &SWAP_PROGRAM_ID,
                    source_token_program_id,
                    destination_token_program_id,
                    &self.pool_token_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &user_transfer_key,
                    user_source_key,
                    swap_source_key,
                    swap_destination_key,
                    user_destination_key,
                    &self.pool_mint_key,
                    &self.pool_fee_key,
                    &source_mint_key,
                    &destination_mint_key,
                    None,
                    Swap {
                        amount_in,
                        minimum_amount_out,
                    },
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    user_source_account,
                    &mut swap_source_account,
                    &mut swap_destination_account,
                    user_destination_account,
                    &mut self.pool_mint_account,
                    &mut self.pool_fee_account,
                    &mut source_mint_account,
                    &mut destination_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )?;

            self.set_token_account(swap_source_key, swap_source_account);
            self.set_token_account(swap_destination_key, swap_destination_account);

            Ok(())
        }

        #[allow(clippy::too_many_arguments)]
        pub fn deposit_all_token_types(
            &mut self,
            depositor_key: &Pubkey,
            depositor_token_a_key: &Pubkey,
            depositor_token_a_account: &mut SolanaAccount,
            depositor_token_b_key: &Pubkey,
            depositor_token_b_account: &mut SolanaAccount,
            depositor_pool_key: &Pubkey,
            depositor_pool_account: &mut SolanaAccount,
            pool_token_amount: u64,
            maximum_token_a_amount: u64,
            maximum_token_b_amount: u64,
        ) -> ProgramResult {
            let user_transfer_authority = Pubkey::new_unique();
            let token_a_program_id = depositor_token_a_account.owner;
            do_process_instruction(
                approve(
                    &token_a_program_id,
                    depositor_token_a_key,
                    &user_transfer_authority,
                    depositor_key,
                    &[],
                    maximum_token_a_amount,
                )
                .unwrap(),
                vec![
                    depositor_token_a_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            let token_b_program_id = depositor_token_b_account.owner;
            do_process_instruction(
                approve(
                    &token_b_program_id,
                    depositor_token_b_key,
                    &user_transfer_authority,
                    depositor_key,
                    &[],
                    maximum_token_b_amount,
                )
                .unwrap(),
                vec![
                    depositor_token_b_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            let pool_token_program_id = depositor_pool_account.owner;
            do_process_instruction(
                deposit_all_token_types(
                    &SWAP_PROGRAM_ID,
                    &token_a_program_id,
                    &token_b_program_id,
                    &pool_token_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &user_transfer_authority,
                    depositor_token_a_key,
                    depositor_token_b_key,
                    &self.token_a_key,
                    &self.token_b_key,
                    &self.pool_mint_key,
                    depositor_pool_key,
                    &self.token_a_mint_key,
                    &self.token_b_mint_key,
                    DepositAllTokenTypes {
                        pool_token_amount,
                        maximum_token_a_amount,
                        maximum_token_b_amount,
                    },
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    depositor_token_a_account,
                    depositor_token_b_account,
                    &mut self.token_a_account,
                    &mut self.token_b_account,
                    &mut self.pool_mint_account,
                    depositor_pool_account,
                    &mut self.token_a_mint_account,
                    &mut self.token_b_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn withdraw_all_token_types(
            &mut self,
            user_key: &Pubkey,
            pool_key: &Pubkey,
            pool_account: &mut SolanaAccount,
            token_a_key: &Pubkey,
            token_a_account: &mut SolanaAccount,
            token_b_key: &Pubkey,
            token_b_account: &mut SolanaAccount,
            pool_token_amount: u64,
            minimum_token_a_amount: u64,
            minimum_token_b_amount: u64,
        ) -> ProgramResult {
            let user_transfer_authority_key = Pubkey::new_unique();
            let pool_token_program_id = pool_account.owner;
            // approve user transfer authority to take out pool tokens
            do_process_instruction(
                approve(
                    &pool_token_program_id,
                    pool_key,
                    &user_transfer_authority_key,
                    user_key,
                    &[],
                    pool_token_amount,
                )
                .unwrap(),
                vec![
                    pool_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            // withdraw token a and b correctly
            let token_a_program_id = token_a_account.owner;
            let token_b_program_id = token_b_account.owner;
            do_process_instruction(
                withdraw_all_token_types(
                    &SWAP_PROGRAM_ID,
                    &pool_token_program_id,
                    &token_a_program_id,
                    &token_b_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &user_transfer_authority_key,
                    &self.pool_mint_key,
                    &self.pool_fee_key,
                    pool_key,
                    &self.token_a_key,
                    &self.token_b_key,
                    token_a_key,
                    token_b_key,
                    &self.token_a_mint_key,
                    &self.token_b_mint_key,
                    WithdrawAllTokenTypes {
                        pool_token_amount,
                        minimum_token_a_amount,
                        minimum_token_b_amount,
                    },
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut self.pool_mint_account,
                    pool_account,
                    &mut self.token_a_account,
                    &mut self.token_b_account,
                    token_a_account,
                    token_b_account,
                    &mut self.pool_fee_account,
                    &mut self.token_a_mint_account,
                    &mut self.token_b_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn deposit_single_token_type_exact_amount_in(
            &mut self,
            depositor_key: &Pubkey,
            deposit_account_key: &Pubkey,
            deposit_token_account: &mut SolanaAccount,
            deposit_pool_key: &Pubkey,
            deposit_pool_account: &mut SolanaAccount,
            source_token_amount: u64,
            minimum_pool_token_amount: u64,
        ) -> ProgramResult {
            let user_transfer_authority_key = Pubkey::new_unique();
            let source_token_program_id = deposit_token_account.owner;
            do_process_instruction(
                approve(
                    &source_token_program_id,
                    deposit_account_key,
                    &user_transfer_authority_key,
                    depositor_key,
                    &[],
                    source_token_amount,
                )
                .unwrap(),
                vec![
                    deposit_token_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            let source_mint_key =
                StateWithExtensions::<Account>::unpack(&deposit_token_account.data)
                    .unwrap()
                    .base
                    .mint;
            let swap_source_key = self.get_swap_key(&source_mint_key);
            let (source_mint_key, mut source_mint_account) = self.get_token_mint(swap_source_key);

            let pool_token_program_id = deposit_pool_account.owner;
            do_process_instruction(
                deposit_single_token_type_exact_amount_in(
                    &SWAP_PROGRAM_ID,
                    &source_token_program_id,
                    &pool_token_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &user_transfer_authority_key,
                    deposit_account_key,
                    &self.token_a_key,
                    &self.token_b_key,
                    &self.pool_mint_key,
                    deposit_pool_key,
                    &source_mint_key,
                    DepositSingleTokenTypeExactAmountIn {
                        source_token_amount,
                        minimum_pool_token_amount,
                    },
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    deposit_token_account,
                    &mut self.token_a_account,
                    &mut self.token_b_account,
                    &mut self.pool_mint_account,
                    deposit_pool_account,
                    &mut source_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
        }

        #[allow(clippy::too_many_arguments)]
        pub fn withdraw_single_token_type_exact_amount_out(
            &mut self,
            user_key: &Pubkey,
            pool_key: &Pubkey,
            pool_account: &mut SolanaAccount,
            destination_key: &Pubkey,
            destination_account: &mut SolanaAccount,
            destination_token_amount: u64,
            maximum_pool_token_amount: u64,
        ) -> ProgramResult {
            let user_transfer_authority_key = Pubkey::new_unique();
            let pool_token_program_id = pool_account.owner;
            // approve user transfer authority to take out pool tokens
            do_process_instruction(
                approve(
                    &pool_token_program_id,
                    pool_key,
                    &user_transfer_authority_key,
                    user_key,
                    &[],
                    maximum_pool_token_amount,
                )
                .unwrap(),
                vec![
                    pool_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();

            let destination_mint_key =
                StateWithExtensions::<Account>::unpack(&destination_account.data)
                    .unwrap()
                    .base
                    .mint;
            let swap_destination_key = self.get_swap_key(&destination_mint_key);
            let (destination_mint_key, mut destination_mint_account) =
                self.get_token_mint(swap_destination_key);

            let destination_token_program_id = destination_account.owner;
            do_process_instruction(
                withdraw_single_token_type_exact_amount_out(
                    &SWAP_PROGRAM_ID,
                    &pool_token_program_id,
                    &destination_token_program_id,
                    &self.swap_key,
                    &self.authority_key,
                    &user_transfer_authority_key,
                    &self.pool_mint_key,
                    &self.pool_fee_key,
                    pool_key,
                    &self.token_a_key,
                    &self.token_b_key,
                    destination_key,
                    &destination_mint_key,
                    WithdrawSingleTokenTypeExactAmountOut {
                        destination_token_amount,
                        maximum_pool_token_amount,
                    },
                )
                .unwrap(),
                vec![
                    &mut self.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut self.pool_mint_account,
                    pool_account,
                    &mut self.token_a_account,
                    &mut self.token_b_account,
                    destination_account,
                    &mut self.pool_fee_account,
                    &mut destination_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
        }
    }

    fn mint_minimum_balance() -> u64 {
        Rent::default().minimum_balance(spl_token::state::Mint::get_packed_len())
    }

    fn account_minimum_balance() -> u64 {
        Rent::default().minimum_balance(spl_token::state::Account::get_packed_len())
    }

    fn do_process_instruction_with_fee_constraints(
        instruction: Instruction,
        accounts: Vec<&mut SolanaAccount>,
        swap_constraints: &Option<SwapConstraints>,
    ) -> ProgramResult {
        test_syscall_stubs();

        // approximate the logic in the actual runtime which runs the instruction
        // and only updates accounts if the instruction is successful
        let mut account_clones = accounts.iter().map(|x| (*x).clone()).collect::<Vec<_>>();
        let mut meta = instruction
            .accounts
            .iter()
            .zip(account_clones.iter_mut())
            .map(|(account_meta, account)| (&account_meta.pubkey, account_meta.is_signer, account))
            .collect::<Vec<_>>();
        let mut account_infos = create_is_signer_account_infos(&mut meta);
        let res = if instruction.program_id == SWAP_PROGRAM_ID {
            Processor::process_with_constraints(
                &instruction.program_id,
                &account_infos,
                &instruction.data,
                swap_constraints,
            )
        } else if instruction.program_id == spl_token::id() {
            spl_token::processor::Processor::process(
                &instruction.program_id,
                &account_infos,
                &instruction.data,
            )
        } else if instruction.program_id == spl_token_2022::id() {
            spl_token_2022::processor::Processor::process(
                &instruction.program_id,
                &account_infos,
                &instruction.data,
            )
        } else {
            Err(ProgramError::IncorrectProgramId)
        };

        if res.is_ok() {
            let mut account_metas = instruction
                .accounts
                .iter()
                .zip(accounts)
                .map(|(account_meta, account)| (&account_meta.pubkey, account))
                .collect::<Vec<_>>();
            for account_info in account_infos.iter_mut() {
                for account_meta in account_metas.iter_mut() {
                    if account_info.key == account_meta.0 {
                        let account = &mut account_meta.1;
                        account.owner = *account_info.owner;
                        account.lamports = **account_info.lamports.borrow();
                        account.data = account_info.data.borrow().to_vec();
                    }
                }
            }
        }
        res
    }

    fn do_process_instruction(
        instruction: Instruction,
        accounts: Vec<&mut SolanaAccount>,
    ) -> ProgramResult {
        do_process_instruction_with_fee_constraints(instruction, accounts, &SWAP_CONSTRAINTS)
    }

    fn mint_token(
        program_id: &Pubkey,
        mint_key: &Pubkey,
        mint_account: &mut SolanaAccount,
        mint_authority_key: &Pubkey,
        account_owner_key: &Pubkey,
        amount: u64,
    ) -> (Pubkey, SolanaAccount) {
        let account_key = Pubkey::new_unique();
        let space = if *program_id == spl_token_2022::id() {
            ExtensionType::try_calculate_account_len::<Account>(&[
                ExtensionType::ImmutableOwner,
                ExtensionType::TransferFeeAmount,
            ])
            .unwrap()
        } else {
            Account::get_packed_len()
        };
        let minimum_balance = Rent::default().minimum_balance(space);
        let mut account_account = SolanaAccount::new(minimum_balance, space, program_id);
        let mut mint_authority_account = SolanaAccount::default();
        let mut rent_sysvar_account = create_account_for_test(&Rent::free());

        // no-ops in normal token, so we're good to run it either way
        do_process_instruction(
            initialize_immutable_owner(program_id, &account_key).unwrap(),
            vec![&mut account_account],
        )
        .unwrap();

        do_process_instruction(
            initialize_account(program_id, &account_key, mint_key, account_owner_key).unwrap(),
            vec![
                &mut account_account,
                mint_account,
                &mut mint_authority_account,
                &mut rent_sysvar_account,
            ],
        )
        .unwrap();

        if amount > 0 {
            do_process_instruction(
                mint_to(
                    program_id,
                    mint_key,
                    &account_key,
                    mint_authority_key,
                    &[],
                    amount,
                )
                .unwrap(),
                vec![
                    mint_account,
                    &mut account_account,
                    &mut mint_authority_account,
                ],
            )
            .unwrap();
        }

        (account_key, account_account)
    }

    fn create_mint(
        program_id: &Pubkey,
        authority_key: &Pubkey,
        freeze_authority: Option<&Pubkey>,
        close_authority: Option<&Pubkey>,
        fees: &TransferFee,
    ) -> (Pubkey, SolanaAccount) {
        let mint_key = Pubkey::new_unique();
        let space = if *program_id == spl_token_2022::id() {
            if close_authority.is_some() {
                ExtensionType::try_calculate_account_len::<Mint>(&[
                    ExtensionType::MintCloseAuthority,
                    ExtensionType::TransferFeeConfig,
                ])
                .unwrap()
            } else {
                ExtensionType::try_calculate_account_len::<Mint>(&[
                    ExtensionType::TransferFeeConfig,
                ])
                .unwrap()
            }
        } else {
            Mint::get_packed_len()
        };
        let minimum_balance = Rent::default().minimum_balance(space);
        let mut mint_account = SolanaAccount::new(minimum_balance, space, program_id);
        let mut rent_sysvar_account = create_account_for_test(&Rent::free());

        if *program_id == spl_token_2022::id() {
            if close_authority.is_some() {
                do_process_instruction(
                    initialize_mint_close_authority(program_id, &mint_key, close_authority)
                        .unwrap(),
                    vec![&mut mint_account],
                )
                .unwrap();
            }
            do_process_instruction(
                initialize_transfer_fee_config(
                    program_id,
                    &mint_key,
                    freeze_authority,
                    freeze_authority,
                    fees.transfer_fee_basis_points.into(),
                    fees.maximum_fee.into(),
                )
                .unwrap(),
                vec![&mut mint_account],
            )
            .unwrap();
        }
        do_process_instruction(
            initialize_mint(program_id, &mint_key, authority_key, freeze_authority, 2).unwrap(),
            vec![&mut mint_account, &mut rent_sysvar_account],
        )
        .unwrap();

        (mint_key, mint_account)
    }

    #[test_case(spl_token::id(); "token")]
    #[test_case(spl_token_2022::id(); "token-2022")]
    fn test_token_program_id_error(token_program_id: Pubkey) {
        test_syscall_stubs();
        let swap_key = Pubkey::new_unique();
        let mut mint = (Pubkey::new_unique(), SolanaAccount::default());
        let mut destination = (Pubkey::new_unique(), SolanaAccount::default());
        let token_program = (token_program_id, SolanaAccount::default());
        let (authority_key, bump_seed) =
            Pubkey::find_program_address(&[&swap_key.to_bytes()[..]], &SWAP_PROGRAM_ID);
        let mut authority = (authority_key, SolanaAccount::default());
        let swap_bytes = swap_key.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[bump_seed]];
        let signers = &[&authority_signature_seeds[..]];
        let ix = mint_to(
            &token_program.0,
            &mint.0,
            &destination.0,
            &authority.0,
            &[],
            10,
        )
        .unwrap();
        let mint = (&mut mint).into();
        let destination = (&mut destination).into();
        let authority = (&mut authority).into();

        let err = invoke_signed(&ix, &[mint, destination, authority], signers).unwrap_err();
        assert_eq!(err, ProgramError::InvalidAccountData);
    }

    #[test_case(spl_token::id(); "token")]
    #[test_case(spl_token_2022::id(); "token-2022")]
    fn test_token_error(token_program_id: Pubkey) {
        test_syscall_stubs();
        let swap_key = Pubkey::new_unique();
        let mut mint = (
            Pubkey::new_unique(),
            SolanaAccount::new(
                mint_minimum_balance(),
                spl_token::state::Mint::get_packed_len(),
                &token_program_id,
            ),
        );
        let mut destination = (
            Pubkey::new_unique(),
            SolanaAccount::new(
                account_minimum_balance(),
                spl_token::state::Account::get_packed_len(),
                &token_program_id,
            ),
        );
        let mut token_program = (token_program_id, SolanaAccount::default());
        let (authority_key, bump_seed) =
            Pubkey::find_program_address(&[&swap_key.to_bytes()[..]], &SWAP_PROGRAM_ID);
        let mut authority = (authority_key, SolanaAccount::default());
        let swap_bytes = swap_key.to_bytes();
        let authority_signature_seeds = [&swap_bytes[..32], &[bump_seed]];
        let signers = &[&authority_signature_seeds[..]];
        let mut rent_sysvar = (
            Pubkey::new_unique(),
            create_account_for_test(&Rent::default()),
        );
        do_process_instruction(
            initialize_mint(
                &token_program.0,
                &mint.0,
                &authority.0,
                Some(&authority.0),
                2,
            )
            .unwrap(),
            vec![&mut mint.1, &mut rent_sysvar.1],
        )
        .unwrap();
        do_process_instruction(
            initialize_account(&token_program.0, &destination.0, &mint.0, &authority.0).unwrap(),
            vec![
                &mut destination.1,
                &mut mint.1,
                &mut authority.1,
                &mut rent_sysvar.1,
                &mut token_program.1,
            ],
        )
        .unwrap();
        do_process_instruction(
            freeze_account(&token_program.0, &destination.0, &mint.0, &authority.0, &[]).unwrap(),
            vec![
                &mut destination.1,
                &mut mint.1,
                &mut authority.1,
                &mut token_program.1,
            ],
        )
        .unwrap();
        let ix = mint_to(
            &token_program.0,
            &mint.0,
            &destination.0,
            &authority.0,
            &[],
            10,
        )
        .unwrap();
        let mint_info = (&mut mint).into();
        let destination_info = (&mut destination).into();
        let authority_info = (&mut authority).into();
        let token_program_info = (&mut token_program).into();

        let err = invoke_signed_wrapper::<TokenError>(
            &ix,
            &[
                mint_info,
                destination_info,
                authority_info,
                token_program_info,
            ],
            signers,
        )
        .unwrap_err();
        assert_eq!(err, ProgramError::Custom(TokenError::AccountFrozen as u32));
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_initialize(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 2;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 20;
        let host_fee_denominator = 100;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 1000;
        let token_b_amount = 2000;
        let pool_token_amount = 10;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        // uninitialized token a account
        {
            let old_account = accounts.token_a_account;
            accounts.token_a_account = SolanaAccount::new(0, 0, &token_a_program_id);
            assert_eq!(
                Err(SwapError::ExpectedAccount.into()),
                accounts.initialize_swap()
            );
            accounts.token_a_account = old_account;
        }

        // uninitialized token b account
        {
            let old_account = accounts.token_b_account;
            accounts.token_b_account = SolanaAccount::new(0, 0, &token_b_program_id);
            assert_eq!(
                Err(SwapError::ExpectedAccount.into()),
                accounts.initialize_swap()
            );
            accounts.token_b_account = old_account;
        }

        // uninitialized pool mint
        {
            let old_account = accounts.pool_mint_account;
            accounts.pool_mint_account = SolanaAccount::new(0, 0, &pool_token_program_id);
            assert_eq!(
                Err(SwapError::ExpectedMint.into()),
                accounts.initialize_swap()
            );
            accounts.pool_mint_account = old_account;
        }

        // token A account owner is not swap authority
        {
            let (_token_a_key, token_a_account) = mint_token(
                &token_a_program_id,
                &accounts.token_a_mint_key,
                &mut accounts.token_a_mint_account,
                &user_key,
                &user_key,
                0,
            );
            let old_account = accounts.token_a_account;
            accounts.token_a_account = token_a_account;
            assert_eq!(
                Err(SwapError::InvalidOwner.into()),
                accounts.initialize_swap()
            );
            accounts.token_a_account = old_account;
        }

        // token B account owner is not swap authority
        {
            let (_token_b_key, token_b_account) = mint_token(
                &token_b_program_id,
                &accounts.token_b_mint_key,
                &mut accounts.token_b_mint_account,
                &user_key,
                &user_key,
                0,
            );
            let old_account = accounts.token_b_account;
            accounts.token_b_account = token_b_account;
            assert_eq!(
                Err(SwapError::InvalidOwner.into()),
                accounts.initialize_swap()
            );
            accounts.token_b_account = old_account;
        }

        // pool token account owner is swap authority
        {
            let (_pool_token_key, pool_token_account) = mint_token(
                &pool_token_program_id,
                &accounts.pool_mint_key,
                &mut accounts.pool_mint_account,
                &accounts.authority_key,
                &accounts.authority_key,
                0,
            );
            let old_account = accounts.pool_token_account;
            accounts.pool_token_account = pool_token_account;
            assert_eq!(
                Err(SwapError::InvalidOutputOwner.into()),
                accounts.initialize_swap()
            );
            accounts.pool_token_account = old_account;
        }

        // pool fee account owner is swap authority
        {
            let (_pool_fee_key, pool_fee_account) = mint_token(
                &pool_token_program_id,
                &accounts.pool_mint_key,
                &mut accounts.pool_mint_account,
                &accounts.authority_key,
                &accounts.authority_key,
                0,
            );
            let old_account = accounts.pool_fee_account;
            accounts.pool_fee_account = pool_fee_account;
            assert_eq!(
                Err(SwapError::InvalidOutputOwner.into()),
                accounts.initialize_swap()
            );
            accounts.pool_fee_account = old_account;
        }

        // pool mint authority is not swap authority
        {
            let (_pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &user_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_mint = accounts.pool_mint_account;
            accounts.pool_mint_account = pool_mint_account;
            assert_eq!(
                Err(SwapError::InvalidOwner.into()),
                accounts.initialize_swap()
            );
            accounts.pool_mint_account = old_mint;
        }

        // pool mint token has freeze authority
        {
            let (_pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                Some(&user_key),
                None,
                &TransferFee::default(),
            );
            let old_mint = accounts.pool_mint_account;
            accounts.pool_mint_account = pool_mint_account;
            assert_eq!(
                Err(SwapError::InvalidFreezeAuthority.into()),
                accounts.initialize_swap()
            );
            accounts.pool_mint_account = old_mint;
        }

        // pool mint token has close authority, only available in token-2022
        if pool_token_program_id == spl_token_2022::id() {
            let (_pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                Some(&user_key),
                &TransferFee::default(),
            );
            let old_mint = accounts.pool_mint_account;
            accounts.pool_mint_account = pool_mint_account;
            assert_eq!(
                Err(SwapError::InvalidCloseAuthority.into()),
                accounts.initialize_swap()
            );
            accounts.pool_mint_account = old_mint;
        }

        // token A account owned by wrong program
        {
            let (_token_a_key, mut token_a_account) = mint_token(
                &token_a_program_id,
                &accounts.token_a_mint_key,
                &mut accounts.token_a_mint_account,
                &user_key,
                &accounts.authority_key,
                token_a_amount,
            );
            token_a_account.owner = SWAP_PROGRAM_ID;
            let old_account = accounts.token_a_account;
            accounts.token_a_account = token_a_account;
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                accounts.initialize_swap()
            );
            accounts.token_a_account = old_account;
        }

        // token B account owned by wrong program
        {
            let (_token_b_key, mut token_b_account) = mint_token(
                &token_b_program_id,
                &accounts.token_b_mint_key,
                &mut accounts.token_b_mint_account,
                &user_key,
                &accounts.authority_key,
                token_b_amount,
            );
            token_b_account.owner = SWAP_PROGRAM_ID;
            let old_account = accounts.token_b_account;
            accounts.token_b_account = token_b_account;
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                accounts.initialize_swap()
            );
            accounts.token_b_account = old_account;
        }

        // empty token A account
        {
            let (_token_a_key, token_a_account) = mint_token(
                &token_a_program_id,
                &accounts.token_a_mint_key,
                &mut accounts.token_a_mint_account,
                &user_key,
                &accounts.authority_key,
                0,
            );
            let old_account = accounts.token_a_account;
            accounts.token_a_account = token_a_account;
            assert_eq!(
                Err(SwapError::EmptySupply.into()),
                accounts.initialize_swap()
            );
            accounts.token_a_account = old_account;
        }

        // empty token B account
        {
            let (_token_b_key, token_b_account) = mint_token(
                &token_b_program_id,
                &accounts.token_b_mint_key,
                &mut accounts.token_b_mint_account,
                &user_key,
                &accounts.authority_key,
                0,
            );
            let old_account = accounts.token_b_account;
            accounts.token_b_account = token_b_account;
            assert_eq!(
                Err(SwapError::EmptySupply.into()),
                accounts.initialize_swap()
            );
            accounts.token_b_account = old_account;
        }

        // invalid pool tokens
        {
            let old_mint = accounts.pool_mint_account;
            let old_pool_account = accounts.pool_token_account;

            let (_pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            accounts.pool_mint_account = pool_mint_account;

            let (_empty_pool_token_key, empty_pool_token_account) = mint_token(
                &pool_token_program_id,
                &accounts.pool_mint_key,
                &mut accounts.pool_mint_account,
                &accounts.authority_key,
                &user_key,
                0,
            );

            let (_pool_token_key, pool_token_account) = mint_token(
                &pool_token_program_id,
                &accounts.pool_mint_key,
                &mut accounts.pool_mint_account,
                &accounts.authority_key,
                &user_key,
                pool_token_amount,
            );

            // non-empty pool token account
            accounts.pool_token_account = pool_token_account;
            assert_eq!(
                Err(SwapError::InvalidSupply.into()),
                accounts.initialize_swap()
            );

            // pool tokens already in circulation
            accounts.pool_token_account = empty_pool_token_account;
            assert_eq!(
                Err(SwapError::InvalidSupply.into()),
                accounts.initialize_swap()
            );

            accounts.pool_mint_account = old_mint;
            accounts.pool_token_account = old_pool_account;
        }

        // pool fee account has wrong mint
        {
            let (_pool_fee_key, pool_fee_account) = mint_token(
                &token_a_program_id,
                &accounts.token_a_mint_key,
                &mut accounts.token_a_mint_account,
                &user_key,
                &user_key,
                0,
            );
            let old_account = accounts.pool_fee_account;
            accounts.pool_fee_account = pool_fee_account;
            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.initialize_swap()
            );
            accounts.pool_fee_account = old_account;
        }

        // token A account is delegated
        {
            do_process_instruction(
                approve(
                    &token_a_program_id,
                    &accounts.token_a_key,
                    &user_key,
                    &accounts.authority_key,
                    &[],
                    1,
                )
                .unwrap(),
                vec![
                    &mut accounts.token_a_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();
            assert_eq!(
                Err(SwapError::InvalidDelegate.into()),
                accounts.initialize_swap()
            );

            do_process_instruction(
                revoke(
                    &token_a_program_id,
                    &accounts.token_a_key,
                    &accounts.authority_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_a_account, &mut SolanaAccount::default()],
            )
            .unwrap();
        }

        // token B account is delegated
        {
            do_process_instruction(
                approve(
                    &token_b_program_id,
                    &accounts.token_b_key,
                    &user_key,
                    &accounts.authority_key,
                    &[],
                    1,
                )
                .unwrap(),
                vec![
                    &mut accounts.token_b_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
            )
            .unwrap();
            assert_eq!(
                Err(SwapError::InvalidDelegate.into()),
                accounts.initialize_swap()
            );

            do_process_instruction(
                revoke(
                    &token_b_program_id,
                    &accounts.token_b_key,
                    &accounts.authority_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_b_account, &mut SolanaAccount::default()],
            )
            .unwrap();
        }

        // token A account has close authority
        {
            do_process_instruction(
                set_authority(
                    &token_a_program_id,
                    &accounts.token_a_key,
                    Some(&user_key),
                    AuthorityType::CloseAccount,
                    &accounts.authority_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_a_account, &mut SolanaAccount::default()],
            )
            .unwrap();
            assert_eq!(
                Err(SwapError::InvalidCloseAuthority.into()),
                accounts.initialize_swap()
            );

            do_process_instruction(
                set_authority(
                    &token_a_program_id,
                    &accounts.token_a_key,
                    None,
                    AuthorityType::CloseAccount,
                    &user_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_a_account, &mut SolanaAccount::default()],
            )
            .unwrap();
        }

        // token B account has close authority
        {
            do_process_instruction(
                set_authority(
                    &token_b_program_id,
                    &accounts.token_b_key,
                    Some(&user_key),
                    AuthorityType::CloseAccount,
                    &accounts.authority_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_b_account, &mut SolanaAccount::default()],
            )
            .unwrap();
            assert_eq!(
                Err(SwapError::InvalidCloseAuthority.into()),
                accounts.initialize_swap()
            );

            do_process_instruction(
                set_authority(
                    &token_b_program_id,
                    &accounts.token_b_key,
                    None,
                    AuthorityType::CloseAccount,
                    &user_key,
                    &[],
                )
                .unwrap(),
                vec![&mut accounts.token_b_account, &mut SolanaAccount::default()],
            )
            .unwrap();
        }

        // wrong token program id
        {
            let wrong_program_id = Pubkey::new_unique();
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                do_process_instruction(
                    initialize(
                        &SWAP_PROGRAM_ID,
                        &wrong_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.pool_token_key,
                        accounts.fees.clone(),
                        accounts.swap_curve.clone(),
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.pool_token_account,
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // create swap with same token A and B
        {
            let (_token_a_repeat_key, token_a_repeat_account) = mint_token(
                &token_a_program_id,
                &accounts.token_a_mint_key,
                &mut accounts.token_a_mint_account,
                &user_key,
                &accounts.authority_key,
                10,
            );
            let old_account = accounts.token_b_account;
            accounts.token_b_account = token_a_repeat_account;
            assert_eq!(
                Err(SwapError::RepeatedMint.into()),
                accounts.initialize_swap()
            );
            accounts.token_b_account = old_account;
        }

        // create valid swap
        accounts.initialize_swap().unwrap();

        // create invalid flat swap
        {
            let token_b_price = 0;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let swap_curve = SwapCurve {
                curve_type: CurveType::ConstantPrice,
                calculator: Arc::new(ConstantPriceCurve { token_b_price }),
            };
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees,
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            assert_eq!(
                Err(SwapError::InvalidCurve.into()),
                accounts.initialize_swap()
            );
        }

        // create valid flat swap
        {
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let token_b_price = 10_000;
            let swap_curve = SwapCurve {
                curve_type: CurveType::ConstantPrice,
                calculator: Arc::new(ConstantPriceCurve { token_b_price }),
            };
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees,
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            accounts.initialize_swap().unwrap();
        }

        // create invalid offset swap
        {
            let token_b_offset = 0;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let swap_curve = SwapCurve {
                curve_type: CurveType::Offset,
                calculator: Arc::new(OffsetCurve { token_b_offset }),
            };
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees,
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            assert_eq!(
                Err(SwapError::InvalidCurve.into()),
                accounts.initialize_swap()
            );
        }

        // create valid offset swap
        {
            let token_b_offset = 10;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let swap_curve = SwapCurve {
                curve_type: CurveType::Offset,
                calculator: Arc::new(OffsetCurve { token_b_offset }),
            };
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees,
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            accounts.initialize_swap().unwrap();
        }

        // wrong owner key in constraint
        {
            let new_key = Pubkey::new_unique();
            let trade_fee_numerator = 25;
            let trade_fee_denominator = 10000;
            let owner_trade_fee_numerator = 5;
            let owner_trade_fee_denominator = 10000;
            let host_fee_numerator = 20;
            let host_fee_denominator = 100;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let curve = ConstantProductCurve {};
            let swap_curve = SwapCurve {
                curve_type: CurveType::ConstantProduct,
                calculator: Arc::new(curve),
            };
            let owner_key = new_key.to_string();
            let valid_curve_types = &[CurveType::ConstantProduct];
            let constraints = Some(SwapConstraints {
                owner_key: Some(owner_key.as_ref()),
                valid_curve_types,
                fees: &fees,
            });
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees.clone(),
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            assert_eq!(
                Err(SwapError::InvalidOwner.into()),
                do_process_instruction_with_fee_constraints(
                    initialize(
                        &SWAP_PROGRAM_ID,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.pool_token_key,
                        accounts.fees.clone(),
                        accounts.swap_curve.clone(),
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.pool_token_account,
                        &mut SolanaAccount::default(),
                    ],
                    &constraints,
                )
            );
        }

        // wrong fee in constraint
        {
            let trade_fee_numerator = 25;
            let trade_fee_denominator = 10000;
            let owner_trade_fee_numerator = 5;
            let owner_trade_fee_denominator = 10000;
            let host_fee_numerator = 20;
            let host_fee_denominator = 100;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let curve = ConstantProductCurve {};
            let swap_curve = SwapCurve {
                curve_type: CurveType::ConstantProduct,
                calculator: Arc::new(curve),
            };
            let owner_key = user_key.to_string();
            let valid_curve_types = &[CurveType::ConstantProduct];
            let constraints = Some(SwapConstraints {
                owner_key: Some(owner_key.as_ref()),
                valid_curve_types,
                fees: &fees,
            });
            let mut bad_fees = fees.clone();
            bad_fees.trade_fee_numerator = trade_fee_numerator - 1;
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                bad_fees,
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            assert_eq!(
                Err(SwapError::InvalidFee.into()),
                do_process_instruction_with_fee_constraints(
                    initialize(
                        &SWAP_PROGRAM_ID,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.pool_token_key,
                        accounts.fees.clone(),
                        accounts.swap_curve.clone(),
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.pool_token_account,
                        &mut SolanaAccount::default(),
                    ],
                    &constraints,
                )
            );
        }

        // create valid swap with constraints
        {
            let trade_fee_numerator = 25;
            let trade_fee_denominator = 10000;
            let owner_trade_fee_numerator = 5;
            let owner_trade_fee_denominator = 10000;
            let host_fee_numerator = 20;
            let host_fee_denominator = 100;
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let curve = ConstantProductCurve {};
            let swap_curve = SwapCurve {
                curve_type: CurveType::ConstantProduct,
                calculator: Arc::new(curve),
            };
            let owner_key = user_key.to_string();
            let valid_curve_types = &[CurveType::ConstantProduct];
            let constraints = Some(SwapConstraints {
                owner_key: Some(owner_key.as_ref()),
                valid_curve_types,
                fees: &fees,
            });
            let mut accounts = SwapAccountInfo::new(
                &user_key,
                fees.clone(),
                SwapTransferFees::default(),
                swap_curve,
                token_a_amount,
                token_b_amount,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
            );
            do_process_instruction_with_fee_constraints(
                initialize(
                    &SWAP_PROGRAM_ID,
                    &pool_token_program_id,
                    &accounts.swap_key,
                    &accounts.authority_key,
                    &accounts.token_a_key,
                    &accounts.token_b_key,
                    &accounts.pool_mint_key,
                    &accounts.pool_fee_key,
                    &accounts.pool_token_key,
                    accounts.fees,
                    accounts.swap_curve.clone(),
                )
                .unwrap(),
                vec![
                    &mut accounts.swap_account,
                    &mut SolanaAccount::default(),
                    &mut accounts.token_a_account,
                    &mut accounts.token_b_account,
                    &mut accounts.pool_mint_account,
                    &mut accounts.pool_fee_account,
                    &mut accounts.pool_token_account,
                    &mut SolanaAccount::default(),
                ],
                &constraints,
            )
            .unwrap();
        }

        // create again
        {
            assert_eq!(
                Err(SwapError::AlreadyInUse.into()),
                accounts.initialize_swap()
            );
        }
        let swap_state = SwapVersion::unpack(&accounts.swap_account.data).unwrap();
        assert!(swap_state.is_initialized());
        assert_eq!(swap_state.bump_seed(), accounts.bump_seed);
        assert_eq!(
            swap_state.swap_curve().curve_type,
            accounts.swap_curve.curve_type
        );
        assert_eq!(*swap_state.token_a_account(), accounts.token_a_key);
        assert_eq!(*swap_state.token_b_account(), accounts.token_b_key);
        assert_eq!(*swap_state.pool_mint(), accounts.pool_mint_key);
        assert_eq!(*swap_state.token_a_mint(), accounts.token_a_mint_key);
        assert_eq!(*swap_state.token_b_mint(), accounts.token_b_mint_key);
        assert_eq!(*swap_state.pool_fee_account(), accounts.pool_fee_key);
        let token_a =
            StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
        assert_eq!(token_a.base.amount, token_a_amount);
        let token_b =
            StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
        assert_eq!(token_b.base.amount, token_b_amount);
        let pool_account =
            StateWithExtensions::<Account>::unpack(&accounts.pool_token_account.data).unwrap();
        let pool_mint =
            StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
        assert_eq!(pool_mint.base.supply, pool_account.base.amount);
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_deposit(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let depositor_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 2;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 20;
        let host_fee_denominator = 100;

        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 1000;
        let token_b_amount = 9000;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        // depositing 10% of the current pool amount in token A and B means
        // that our pool tokens will be worth 1 / 10 of the current pool amount
        let pool_amount = INITIAL_SWAP_POOL_AMOUNT / 10;
        let deposit_a = token_a_amount / 10;
        let deposit_b = token_b_amount / 10;

        // swap not initialized
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            assert_eq!(
                Err(ProgramError::UninitializedAccount),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        accounts.initialize_swap().unwrap();

        // wrong owner for swap account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let old_swap_account = accounts.swap_account;
            let mut wrong_swap_account = old_swap_account.clone();
            wrong_swap_account.owner = pool_token_program_id;
            accounts.swap_account = wrong_swap_account;
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
            accounts.swap_account = old_swap_account;
        }

        // wrong bump seed for authority_key
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let old_authority = accounts.authority_key;
            let (bad_authority_key, _bump_seed) = Pubkey::find_program_address(
                &[&accounts.swap_key.to_bytes()[..]],
                &pool_token_program_id,
            );
            accounts.authority_key = bad_authority_key;
            assert_eq!(
                Err(SwapError::InvalidProgramAddress.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
            accounts.authority_key = old_authority;
        }

        // not enough token A
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &depositor_key,
                deposit_a / 2,
                deposit_b,
                0,
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // not enough token B
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &depositor_key,
                deposit_a,
                deposit_b / 2,
                0,
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // wrong swap token accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let expected_error: ProgramError = if token_a_account.owner == token_b_account.owner {
                TokenError::MintMismatch.into()
            } else {
                ProgramError::InvalidAccountData
            };
            assert_eq!(
                Err(expected_error),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_b_key,
                    &mut token_b_account,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // wrong pool token account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                mut _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let (
                wrong_token_key,
                mut wrong_token_account,
                _token_b_key,
                mut _token_b_account,
                _pool_key,
                pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let expected_error: ProgramError = if token_a_account.owner == pool_account.owner {
                TokenError::MintMismatch.into()
            } else {
                SwapError::IncorrectTokenProgramId.into()
            };
            assert_eq!(
                Err(expected_error),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &wrong_token_key,
                    &mut wrong_token_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // no approval
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let user_transfer_authority_key = Pubkey::new_unique();
            assert_eq!(
                Err(TokenError::OwnerMismatch.into()),
                do_process_instruction(
                    deposit_all_token_types(
                        &SWAP_PROGRAM_ID,
                        &token_a_program_id,
                        &token_b_program_id,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_authority_key,
                        &token_a_key,
                        &token_b_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &pool_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        DepositAllTokenTypes {
                            pool_token_amount: pool_amount.try_into().unwrap(),
                            maximum_token_a_amount: deposit_a,
                            maximum_token_b_amount: deposit_b,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut token_b_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong token program id
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let wrong_key = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                do_process_instruction(
                    deposit_all_token_types(
                        &SWAP_PROGRAM_ID,
                        &wrong_key,
                        &wrong_key,
                        &wrong_key,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &token_a_key,
                        &token_b_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &pool_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        DepositAllTokenTypes {
                            pool_token_amount: pool_amount.try_into().unwrap(),
                            maximum_token_a_amount: deposit_a,
                            maximum_token_b_amount: deposit_b,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut token_b_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong swap token accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);

            let old_a_key = accounts.token_a_key;
            let old_a_account = accounts.token_a_account;

            accounts.token_a_key = token_a_key;
            accounts.token_a_account = token_a_account.clone();

            // wrong swap token a account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );

            accounts.token_a_key = old_a_key;
            accounts.token_a_account = old_a_account;

            let old_b_key = accounts.token_b_key;
            let old_b_account = accounts.token_b_account;

            accounts.token_b_key = token_b_key;
            accounts.token_b_account = token_b_account.clone();

            // wrong swap token b account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );

            accounts.token_b_key = old_b_key;
            accounts.token_b_account = old_b_account;
        }

        // wrong mint
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let (pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_pool_key = accounts.pool_mint_key;
            let old_pool_account = accounts.pool_mint_account;
            accounts.pool_mint_key = pool_mint_key;
            accounts.pool_mint_account = pool_mint_account;

            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );

            accounts.pool_mint_key = old_pool_key;
            accounts.pool_mint_account = old_pool_account;
        }

        // deposit 1 pool token fails because it equates to 0 swap tokens
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            assert_eq!(
                Err(SwapError::ZeroTradingTokens.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    1,
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // slippage exceeded
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            // maximum A amount in too low
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a / 10,
                    deposit_b,
                )
            );
            // maximum B amount in too low
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b / 10,
                )
            );
        }

        // invalid input: can't use swap pool tokens as source
        {
            let (
                _token_a_key,
                _token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let swap_token_a_key = accounts.token_a_key;
            let mut swap_token_a_account = accounts.get_token_account(&swap_token_a_key).clone();
            let swap_token_b_key = accounts.token_b_key;
            let mut swap_token_b_account = accounts.get_token_account(&swap_token_b_key).clone();
            let authority_key = accounts.authority_key;
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.deposit_all_token_types(
                    &authority_key,
                    &swap_token_a_key,
                    &mut swap_token_a_account,
                    &swap_token_b_key,
                    &mut swap_token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
            );
        }

        // correctly deposit
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            accounts
                .deposit_all_token_types(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount.try_into().unwrap(),
                    deposit_a,
                    deposit_b,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
            assert_eq!(swap_token_a.base.amount, deposit_a + token_a_amount);
            let swap_token_b =
                StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
            assert_eq!(swap_token_b.base.amount, deposit_b + token_b_amount);
            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(token_a.base.amount, 0);
            let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
            assert_eq!(token_b.base.amount, 0);
            let pool_account = StateWithExtensions::<Account>::unpack(&pool_account.data).unwrap();
            let swap_pool_account =
                StateWithExtensions::<Account>::unpack(&accounts.pool_token_account.data).unwrap();
            let pool_mint =
                StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
            assert_eq!(
                pool_mint.base.supply,
                pool_account.base.amount + swap_pool_account.base.amount
            );
        }
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 2;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 7;
        let host_fee_denominator = 100;

        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 1000;
        let token_b_amount = 2000;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let withdrawer_key = Pubkey::new_unique();
        let initial_a = token_a_amount / 10;
        let initial_b = token_b_amount / 10;
        let initial_pool = swap_curve.calculator.new_pool_supply() / 10;
        let withdraw_amount = initial_pool / 4;
        let minimum_token_a_amount = initial_a / 40;
        let minimum_token_b_amount = initial_b / 40;

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        // swap not initialized
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(ProgramError::UninitializedAccount),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
        }

        accounts.initialize_swap().unwrap();

        // wrong owner for swap account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            let old_swap_account = accounts.swap_account;
            let mut wrong_swap_account = old_swap_account.clone();
            wrong_swap_account.owner = pool_token_program_id;
            accounts.swap_account = wrong_swap_account;
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
            accounts.swap_account = old_swap_account;
        }

        // wrong bump seed for authority_key
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            let old_authority = accounts.authority_key;
            let (bad_authority_key, _bump_seed) = Pubkey::find_program_address(
                &[&accounts.swap_key.to_bytes()[..]],
                &pool_token_program_id,
            );
            accounts.authority_key = bad_authority_key;
            assert_eq!(
                Err(SwapError::InvalidProgramAddress.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
            accounts.authority_key = old_authority;
        }

        // not enough pool tokens
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                to_u64(withdraw_amount).unwrap() / 2u64,
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount / 2,
                    minimum_token_b_amount / 2,
                )
            );
        }

        // wrong token a / b accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let expected_error: ProgramError = if token_a_account.owner == token_b_account.owner {
                TokenError::MintMismatch.into()
            } else {
                ProgramError::InvalidAccountData
            };
            assert_eq!(
                Err(expected_error),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_b_key,
                    &mut token_b_account,
                    &token_a_key,
                    &mut token_a_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
        }

        // wrong pool token account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let (
                wrong_token_a_key,
                mut wrong_token_a_account,
                _token_b_key,
                _token_b_account,
                _pool_key,
                pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                withdraw_amount.try_into().unwrap(),
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let expected_error: ProgramError = if token_a_account.owner == pool_account.owner {
                TokenError::MintMismatch.into()
            } else {
                SwapError::IncorrectTokenProgramId.into()
            };
            assert_eq!(
                Err(expected_error),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &wrong_token_a_key,
                    &mut wrong_token_a_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
        }

        // wrong pool fee account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                wrong_pool_key,
                wrong_pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let (
                _token_a_key,
                _token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let old_pool_fee_account = accounts.pool_fee_account;
            let old_pool_fee_key = accounts.pool_fee_key;
            accounts.pool_fee_account = wrong_pool_account;
            accounts.pool_fee_key = wrong_pool_key;
            assert_eq!(
                Err(SwapError::IncorrectFeeAccount.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                ),
            );
            accounts.pool_fee_account = old_pool_fee_account;
            accounts.pool_fee_key = old_pool_fee_key;
        }

        // no approval
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                0,
                0,
                withdraw_amount.try_into().unwrap(),
            );
            let user_transfer_authority_key = Pubkey::new_unique();
            assert_eq!(
                Err(TokenError::OwnerMismatch.into()),
                do_process_instruction(
                    withdraw_all_token_types(
                        &SWAP_PROGRAM_ID,
                        &pool_token_program_id,
                        &token_a_program_id,
                        &token_b_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_authority_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &pool_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_a_key,
                        &token_b_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        WithdrawAllTokenTypes {
                            pool_token_amount: withdraw_amount.try_into().unwrap(),
                            minimum_token_a_amount,
                            minimum_token_b_amount,
                        }
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_a_account,
                        &mut token_b_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong token program id
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                withdraw_amount.try_into().unwrap(),
            );
            let wrong_key = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                do_process_instruction(
                    withdraw_all_token_types(
                        &SWAP_PROGRAM_ID,
                        &wrong_key,
                        &wrong_key,
                        &wrong_key,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &pool_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_a_key,
                        &token_b_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        WithdrawAllTokenTypes {
                            pool_token_amount: withdraw_amount.try_into().unwrap(),
                            minimum_token_a_amount,
                            minimum_token_b_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_a_account,
                        &mut token_b_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong swap token accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );

            let old_a_key = accounts.token_a_key;
            let old_a_account = accounts.token_a_account;

            accounts.token_a_key = token_a_key;
            accounts.token_a_account = token_a_account.clone();

            // wrong swap token a account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );

            accounts.token_a_key = old_a_key;
            accounts.token_a_account = old_a_account;

            let old_b_key = accounts.token_b_key;
            let old_b_account = accounts.token_b_account;

            accounts.token_b_key = token_b_key;
            accounts.token_b_account = token_b_account.clone();

            // wrong swap token b account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );

            accounts.token_b_key = old_b_key;
            accounts.token_b_account = old_b_account;
        }

        // wrong mint
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );
            let (pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_pool_key = accounts.pool_mint_key;
            let old_pool_account = accounts.pool_mint_account;
            accounts.pool_mint_key = pool_mint_key;
            accounts.pool_mint_account = pool_mint_account;

            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );

            accounts.pool_mint_key = old_pool_key;
            accounts.pool_mint_account = old_pool_account;
        }

        // withdrawing 1 pool token fails because it equates to 0 output tokens
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );
            assert_eq!(
                Err(SwapError::ZeroTradingTokens.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    1,
                    0,
                    0,
                )
            );
        }

        // slippage exceeded
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );
            // minimum A amount out too high
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount * 10,
                    minimum_token_b_amount,
                )
            );
            // minimum B amount out too high
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount * 10,
                )
            );
        }

        // invalid input: can't use swap pool tokens as destination
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );
            let swap_token_a_key = accounts.token_a_key;
            let mut swap_token_a_account = accounts.get_token_account(&swap_token_a_key).clone();
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &swap_token_a_key,
                    &mut swap_token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
            let swap_token_b_key = accounts.token_b_key;
            let mut swap_token_b_account = accounts.get_token_account(&swap_token_b_key).clone();
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_b_key,
                    &mut swap_token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
            );
        }

        // correct withdrawal
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );

            accounts
                .withdraw_all_token_types(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    withdraw_amount.try_into().unwrap(),
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
            let swap_token_b =
                StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
            let pool_mint =
                StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
            let withdraw_fee = accounts.fees.owner_withdraw_fee(withdraw_amount).unwrap();
            let results = accounts
                .swap_curve
                .calculator
                .pool_tokens_to_trading_tokens(
                    withdraw_amount - withdraw_fee,
                    pool_mint.base.supply.into(),
                    swap_token_a.base.amount.into(),
                    swap_token_b.base.amount.into(),
                    RoundDirection::Floor,
                )
                .unwrap();
            assert_eq!(
                swap_token_a.base.amount,
                token_a_amount - to_u64(results.token_a_amount).unwrap()
            );
            assert_eq!(
                swap_token_b.base.amount,
                token_b_amount - to_u64(results.token_b_amount).unwrap()
            );
            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(
                token_a.base.amount,
                initial_a + to_u64(results.token_a_amount).unwrap()
            );
            let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
            assert_eq!(
                token_b.base.amount,
                initial_b + to_u64(results.token_b_amount).unwrap()
            );
            let pool_account = StateWithExtensions::<Account>::unpack(&pool_account.data).unwrap();
            assert_eq!(
                pool_account.base.amount,
                to_u64(initial_pool - withdraw_amount).unwrap()
            );
            let fee_account =
                StateWithExtensions::<Account>::unpack(&accounts.pool_fee_account.data).unwrap();
            assert_eq!(
                fee_account.base.amount,
                TryInto::<u64>::try_into(withdraw_fee).unwrap()
            );
        }

        // correct withdrawal from fee account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                mut _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, 0, 0, 0);

            let pool_fee_key = accounts.pool_fee_key;
            let mut pool_fee_account = accounts.pool_fee_account.clone();
            let fee_account =
                StateWithExtensions::<Account>::unpack(&pool_fee_account.data).unwrap();
            let pool_fee_amount = fee_account.base.amount;

            accounts
                .withdraw_all_token_types(
                    &user_key,
                    &pool_fee_key,
                    &mut pool_fee_account,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    pool_fee_amount,
                    0,
                    0,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
            let swap_token_b =
                StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
            let pool_mint =
                StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
            let results = accounts
                .swap_curve
                .calculator
                .pool_tokens_to_trading_tokens(
                    pool_fee_amount.into(),
                    pool_mint.base.supply.into(),
                    swap_token_a.base.amount.into(),
                    swap_token_b.base.amount.into(),
                    RoundDirection::Floor,
                )
                .unwrap();
            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(
                token_a.base.amount,
                TryInto::<u64>::try_into(results.token_a_amount).unwrap()
            );
            let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
            assert_eq!(
                token_b.base.amount,
                TryInto::<u64>::try_into(results.token_b_amount).unwrap()
            );
        }
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_deposit_one_exact_in(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let depositor_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 2;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 20;
        let host_fee_denominator = 100;

        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 1000;
        let token_b_amount = 9000;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        let deposit_a = token_a_amount / 10;
        let deposit_b = token_b_amount / 10;
        let pool_amount = to_u64(INITIAL_SWAP_POOL_AMOUNT / 100).unwrap();

        // swap not initialized
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            assert_eq!(
                Err(ProgramError::UninitializedAccount),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );
        }

        accounts.initialize_swap().unwrap();

        // wrong owner for swap account
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let old_swap_account = accounts.swap_account;
            let mut wrong_swap_account = old_swap_account.clone();
            wrong_swap_account.owner = pool_token_program_id;
            accounts.swap_account = wrong_swap_account;
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );
            accounts.swap_account = old_swap_account;
        }

        // wrong bump seed for authority_key
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let old_authority = accounts.authority_key;
            let (bad_authority_key, _bump_seed) = Pubkey::find_program_address(
                &[&accounts.swap_key.to_bytes()[..]],
                &pool_token_program_id,
            );
            accounts.authority_key = bad_authority_key;
            assert_eq!(
                Err(SwapError::InvalidProgramAddress.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );
            accounts.authority_key = old_authority;
        }

        // not enough token A / B
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &depositor_key,
                deposit_a / 2,
                deposit_b / 2,
                0,
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    0,
                )
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_b,
                    0,
                )
            );
        }

        // wrong pool token account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let expected_error: ProgramError = if token_b_account.owner == pool_account.owner {
                TokenError::MintMismatch.into()
            } else {
                SwapError::IncorrectTokenProgramId.into()
            };
            assert_eq!(
                Err(expected_error),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    deposit_a,
                    pool_amount,
                )
            );
        }

        // no approval
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let user_transfer_authority_key = Pubkey::new_unique();
            assert_eq!(
                Err(TokenError::OwnerMismatch.into()),
                do_process_instruction(
                    deposit_single_token_type_exact_amount_in(
                        &SWAP_PROGRAM_ID,
                        &token_a_program_id,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_authority_key,
                        &token_a_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &pool_key,
                        &accounts.token_a_mint_key,
                        DepositSingleTokenTypeExactAmountIn {
                            source_token_amount: deposit_a,
                            minimum_pool_token_amount: pool_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong token program id
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let wrong_key = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                do_process_instruction(
                    deposit_single_token_type_exact_amount_in(
                        &SWAP_PROGRAM_ID,
                        &wrong_key,
                        &wrong_key,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &token_a_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &accounts.pool_mint_key,
                        &pool_key,
                        &accounts.token_a_mint_key,
                        DepositSingleTokenTypeExactAmountIn {
                            source_token_amount: deposit_a,
                            minimum_pool_token_amount: pool_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong swap token accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);

            let old_a_key = accounts.token_a_key;
            let old_a_account = accounts.token_a_account;

            accounts.token_a_key = token_a_key;
            accounts.token_a_account = token_a_account.clone();

            // wrong swap token a account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );

            accounts.token_a_key = old_a_key;
            accounts.token_a_account = old_a_account;

            let old_b_key = accounts.token_b_key;
            let old_b_account = accounts.token_b_account;

            accounts.token_b_key = token_b_key;
            accounts.token_b_account = token_b_account;

            // wrong swap token b account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );

            accounts.token_b_key = old_b_key;
            accounts.token_b_account = old_b_account;
        }

        // wrong mint
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let (pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_pool_key = accounts.pool_mint_key;
            let old_pool_account = accounts.pool_mint_account;
            accounts.pool_mint_key = pool_mint_key;
            accounts.pool_mint_account = pool_mint_account;

            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );

            accounts.pool_mint_key = old_pool_key;
            accounts.pool_mint_account = old_pool_account;
        }

        // slippage exceeded
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            // minimum pool amount too high
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a / 10,
                    pool_amount,
                )
            );
            // minimum pool amount too high
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_b / 10,
                    pool_amount,
                )
            );
        }

        // invalid input: can't use swap pool tokens as source
        {
            let (
                _token_a_key,
                _token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            let swap_token_a_key = accounts.token_a_key;
            let mut swap_token_a_account = accounts.get_token_account(&swap_token_a_key).clone();
            let swap_token_b_key = accounts.token_b_key;
            let mut swap_token_b_account = accounts.get_token_account(&swap_token_b_key).clone();
            let authority_key = accounts.authority_key;
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &authority_key,
                    &swap_token_a_key,
                    &mut swap_token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
            );
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.deposit_single_token_type_exact_amount_in(
                    &authority_key,
                    &swap_token_b_key,
                    &mut swap_token_b_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_b,
                    pool_amount,
                )
            );
        }

        // correctly deposit
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &depositor_key, deposit_a, deposit_b, 0);
            accounts
                .deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_a_key,
                    &mut token_a_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_a,
                    pool_amount,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
            assert_eq!(swap_token_a.base.amount, deposit_a + token_a_amount);

            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(token_a.base.amount, 0);

            accounts
                .deposit_single_token_type_exact_amount_in(
                    &depositor_key,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    deposit_b,
                    pool_amount,
                )
                .unwrap();
            let swap_token_b =
                StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
            assert_eq!(swap_token_b.base.amount, deposit_b + token_b_amount);

            let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
            assert_eq!(token_b.base.amount, 0);

            let pool_account = StateWithExtensions::<Account>::unpack(&pool_account.data).unwrap();
            let swap_pool_account =
                StateWithExtensions::<Account>::unpack(&accounts.pool_token_account.data).unwrap();
            let pool_mint =
                StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
            assert_eq!(
                pool_mint.base.supply,
                pool_account.base.amount + swap_pool_account.base.amount
            );
        }
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw_one_exact_out(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 2;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 7;
        let host_fee_denominator = 100;

        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 100_000;
        let token_b_amount = 200_000;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let withdrawer_key = Pubkey::new_unique();
        let initial_a = token_a_amount / 10;
        let initial_b = token_b_amount / 10;
        let initial_pool = swap_curve.calculator.new_pool_supply() / 10;
        let maximum_pool_token_amount = to_u64(initial_pool / 4).unwrap();
        let destination_a_amount = initial_a / 40;
        let destination_b_amount = initial_b / 40;

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        // swap not initialized
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(ProgramError::UninitializedAccount),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );
        }

        accounts.initialize_swap().unwrap();

        // wrong owner for swap account
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            let old_swap_account = accounts.swap_account;
            let mut wrong_swap_account = old_swap_account.clone();
            wrong_swap_account.owner = pool_token_program_id;
            accounts.swap_account = wrong_swap_account;
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );
            accounts.swap_account = old_swap_account;
        }

        // wrong bump seed for authority_key
        {
            let (
                _token_a_key,
                _token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);
            let old_authority = accounts.authority_key;
            let (bad_authority_key, _bump_seed) = Pubkey::find_program_address(
                &[&accounts.swap_key.to_bytes()[..]],
                &pool_token_program_id,
            );
            accounts.authority_key = bad_authority_key;
            assert_eq!(
                Err(SwapError::InvalidProgramAddress.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_b_key,
                    &mut token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount,
                )
            );
            accounts.authority_key = old_authority;
        }

        // not enough pool tokens
        {
            let (
                _token_a_key,
                _token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount / 1000,
            );
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_b_key,
                    &mut token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount,
                )
            );
        }

        // wrong pool token account
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                maximum_pool_token_amount,
                initial_b,
                maximum_pool_token_amount,
            );
            let expected_error: ProgramError = if token_a_account.owner == pool_account.owner {
                TokenError::MintMismatch.into()
            } else {
                SwapError::IncorrectTokenProgramId.into()
            };
            assert_eq!(
                Err(expected_error),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount,
                )
            );
        }

        // wrong pool fee account
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                wrong_pool_key,
                wrong_pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount,
            );
            let (
                _token_a_key,
                _token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount,
            );
            let old_pool_fee_account = accounts.pool_fee_account;
            let old_pool_fee_key = accounts.pool_fee_key;
            accounts.pool_fee_account = wrong_pool_account;
            accounts.pool_fee_key = wrong_pool_key;
            assert_eq!(
                Err(SwapError::IncorrectFeeAccount.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );
            accounts.pool_fee_account = old_pool_fee_account;
            accounts.pool_fee_key = old_pool_fee_key;
        }

        // no approval
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                0,
                0,
                maximum_pool_token_amount,
            );
            let user_transfer_authority_key = Pubkey::new_unique();
            assert_eq!(
                Err(TokenError::OwnerMismatch.into()),
                do_process_instruction(
                    withdraw_single_token_type_exact_amount_out(
                        &SWAP_PROGRAM_ID,
                        &pool_token_program_id,
                        &token_a_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_authority_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &pool_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_a_key,
                        &accounts.token_a_mint_key,
                        WithdrawSingleTokenTypeExactAmountOut {
                            destination_token_amount: destination_a_amount,
                            maximum_pool_token_amount,
                        }
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_a_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong token program id
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount,
            );
            let wrong_key = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                do_process_instruction(
                    withdraw_single_token_type_exact_amount_out(
                        &SWAP_PROGRAM_ID,
                        &wrong_key,
                        &wrong_key,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &pool_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_a_key,
                        &accounts.token_a_mint_key,
                        WithdrawSingleTokenTypeExactAmountOut {
                            destination_token_amount: destination_a_amount,
                            maximum_pool_token_amount,
                        }
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut accounts.pool_mint_account,
                        &mut pool_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_a_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                )
            );
        }

        // wrong swap token accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );

            let old_a_key = accounts.token_a_key;
            let old_a_account = accounts.token_a_account;

            accounts.token_a_key = token_a_key;
            accounts.token_a_account = token_a_account.clone();

            // wrong swap token a account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );

            accounts.token_a_key = old_a_key;
            accounts.token_a_account = old_a_account;

            let old_b_key = accounts.token_b_key;
            let old_b_account = accounts.token_b_account;

            accounts.token_b_key = token_b_key;
            accounts.token_b_account = token_b_account.clone();

            // wrong swap token b account
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_b_key,
                    &mut token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount,
                )
            );

            accounts.token_b_key = old_b_key;
            accounts.token_b_account = old_b_account;
        }

        // wrong mint
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );
            let (pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_pool_key = accounts.pool_mint_key;
            let old_pool_account = accounts.pool_mint_account;
            accounts.pool_mint_key = pool_mint_key;
            accounts.pool_mint_account = pool_mint_account;

            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );

            accounts.pool_mint_key = old_pool_key;
            accounts.pool_mint_account = old_pool_account;
        }

        // slippage exceeded
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount,
            );

            // maximum pool token amount too low
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount / 1000,
                )
            );
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_b_key,
                    &mut token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount / 1000,
                )
            );
        }

        // invalid input: can't use swap pool tokens as destination
        {
            let (
                _token_a_key,
                _token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                maximum_pool_token_amount,
            );
            let swap_token_a_key = accounts.token_a_key;
            let mut swap_token_a_account = accounts.get_token_account(&swap_token_a_key).clone();
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &swap_token_a_key,
                    &mut swap_token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
            );
            let swap_token_b_key = accounts.token_b_key;
            let mut swap_token_b_account = accounts.get_token_account(&swap_token_b_key).clone();
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &swap_token_b_key,
                    &mut swap_token_b_account,
                    destination_b_amount,
                    maximum_pool_token_amount,
                )
            );
        }

        // correct withdrawal
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(
                &user_key,
                &withdrawer_key,
                initial_a,
                initial_b,
                initial_pool.try_into().unwrap(),
            );

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
            let swap_token_b =
                StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
            let pool_mint =
                StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();

            let pool_token_amount = accounts
                .swap_curve
                .withdraw_single_token_type_exact_out(
                    destination_a_amount.into(),
                    swap_token_a.base.amount.into(),
                    swap_token_b.base.amount.into(),
                    pool_mint.base.supply.into(),
                    TradeDirection::AtoB,
                    &accounts.fees,
                )
                .unwrap();
            let withdraw_fee = accounts.fees.owner_withdraw_fee(pool_token_amount).unwrap();

            accounts
                .withdraw_single_token_type_exact_amount_out(
                    &withdrawer_key,
                    &pool_key,
                    &mut pool_account,
                    &token_a_key,
                    &mut token_a_account,
                    destination_a_amount,
                    maximum_pool_token_amount,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();

            assert_eq!(
                swap_token_a.base.amount,
                token_a_amount - destination_a_amount
            );
            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(token_a.base.amount, initial_a + destination_a_amount);

            let pool_account = StateWithExtensions::<Account>::unpack(&pool_account.data).unwrap();
            assert_eq!(
                pool_account.base.amount,
                to_u64(initial_pool - pool_token_amount - withdraw_fee).unwrap()
            );
            let fee_account =
                StateWithExtensions::<Account>::unpack(&accounts.pool_fee_account.data).unwrap();
            assert_eq!(fee_account.base.amount, to_u64(withdraw_fee).unwrap());
        }

        // correct withdrawal from fee account
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, initial_a, initial_b, 0);

            let fee_a_amount = 2;
            let pool_fee_key = accounts.pool_fee_key;
            let mut pool_fee_account = accounts.pool_fee_account.clone();
            let fee_account =
                StateWithExtensions::<Account>::unpack(&pool_fee_account.data).unwrap();
            let pool_fee_amount = fee_account.base.amount;

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();

            let token_a_amount = swap_token_a.base.amount;
            accounts
                .withdraw_single_token_type_exact_amount_out(
                    &user_key,
                    &pool_fee_key,
                    &mut pool_fee_account,
                    &token_a_key,
                    &mut token_a_account,
                    fee_a_amount,
                    pool_fee_amount,
                )
                .unwrap();

            let swap_token_a =
                StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();

            assert_eq!(swap_token_a.base.amount, token_a_amount - fee_a_amount);
            let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
            assert_eq!(token_a.base.amount, initial_a + fee_a_amount);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn check_valid_swap_curve(
        fees: Fees,
        transfer_fees: SwapTransferFees,
        curve_type: CurveType,
        calculator: Arc<dyn CurveCalculator + Send + Sync>,
        token_a_amount: u64,
        token_b_amount: u64,
        pool_token_program_id: &Pubkey,
        token_a_program_id: &Pubkey,
        token_b_program_id: &Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let swapper_key = Pubkey::new_unique();

        let swap_curve = SwapCurve {
            curve_type,
            calculator,
        };

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees.clone(),
            transfer_fees,
            swap_curve.clone(),
            token_a_amount,
            token_b_amount,
            pool_token_program_id,
            token_a_program_id,
            token_b_program_id,
        );
        let initial_a = token_a_amount / 5;
        let initial_b = token_b_amount / 5;
        accounts.initialize_swap().unwrap();

        let swap_token_a_key = accounts.token_a_key;
        let swap_token_b_key = accounts.token_b_key;

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            _pool_key,
            _pool_account,
        ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
        // swap one way
        let a_to_b_amount = initial_a / 10;
        let minimum_token_b_amount = 0;
        let pool_mint =
            StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
        let initial_supply = pool_mint.base.supply;
        accounts
            .swap(
                &swapper_key,
                &token_a_key,
                &mut token_a_account,
                &swap_token_a_key,
                &swap_token_b_key,
                &token_b_key,
                &mut token_b_account,
                a_to_b_amount,
                minimum_token_b_amount,
            )
            .unwrap();

        // tweak values based on transfer fees assessed
        let token_a_fee = accounts
            .transfer_fees
            .token_a
            .calculate_fee(a_to_b_amount)
            .unwrap();
        let actual_a_to_b_amount = a_to_b_amount - token_a_fee;
        let results = swap_curve
            .swap(
                actual_a_to_b_amount.into(),
                token_a_amount.into(),
                token_b_amount.into(),
                TradeDirection::AtoB,
                &fees,
            )
            .unwrap();

        let swap_token_a =
            StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
        let token_a_amount = swap_token_a.base.amount;
        assert_eq!(
            token_a_amount,
            TryInto::<u64>::try_into(results.new_swap_source_amount).unwrap()
        );
        let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
        assert_eq!(token_a.base.amount, initial_a - a_to_b_amount);

        let swap_token_b =
            StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
        let token_b_amount = swap_token_b.base.amount;
        assert_eq!(
            token_b_amount,
            TryInto::<u64>::try_into(results.new_swap_destination_amount).unwrap()
        );
        let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
        assert_eq!(
            token_b.base.amount,
            initial_b + to_u64(results.destination_amount_swapped).unwrap()
        );

        let first_fee = if results.owner_fee > 0 {
            swap_curve
                .calculator
                .withdraw_single_token_type_exact_out(
                    results.owner_fee,
                    token_a_amount.into(),
                    token_b_amount.into(),
                    initial_supply.into(),
                    TradeDirection::AtoB,
                    RoundDirection::Floor,
                )
                .unwrap()
        } else {
            0
        };
        let fee_account =
            StateWithExtensions::<Account>::unpack(&accounts.pool_fee_account.data).unwrap();
        assert_eq!(
            fee_account.base.amount,
            TryInto::<u64>::try_into(first_fee).unwrap()
        );

        let first_swap_amount = results.destination_amount_swapped;

        // swap the other way
        let pool_mint =
            StateWithExtensions::<Mint>::unpack(&accounts.pool_mint_account.data).unwrap();
        let initial_supply = pool_mint.base.supply;

        let b_to_a_amount = initial_b / 10;
        let minimum_a_amount = 0;
        accounts
            .swap(
                &swapper_key,
                &token_b_key,
                &mut token_b_account,
                &swap_token_b_key,
                &swap_token_a_key,
                &token_a_key,
                &mut token_a_account,
                b_to_a_amount,
                minimum_a_amount,
            )
            .unwrap();

        let mut results = swap_curve
            .swap(
                b_to_a_amount.into(),
                token_b_amount.into(),
                token_a_amount.into(),
                TradeDirection::BtoA,
                &fees,
            )
            .unwrap();
        // tweak values based on transfer fees assessed
        let token_a_fee = accounts
            .transfer_fees
            .token_a
            .calculate_fee(results.destination_amount_swapped.try_into().unwrap())
            .unwrap();
        results.destination_amount_swapped -= token_a_fee as u128;

        let swap_token_a =
            StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
        let token_a_amount = swap_token_a.base.amount;
        assert_eq!(
            token_a_amount,
            TryInto::<u64>::try_into(results.new_swap_destination_amount).unwrap()
        );
        let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
        assert_eq!(
            token_a.base.amount,
            initial_a - a_to_b_amount + to_u64(results.destination_amount_swapped).unwrap()
        );

        let swap_token_b =
            StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
        let token_b_amount = swap_token_b.base.amount;
        assert_eq!(
            token_b_amount,
            TryInto::<u64>::try_into(results.new_swap_source_amount).unwrap()
        );
        let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
        assert_eq!(
            token_b.base.amount,
            initial_b + to_u64(first_swap_amount).unwrap()
                - to_u64(results.source_amount_swapped).unwrap()
        );

        let second_fee = if results.owner_fee > 0 {
            swap_curve
                .calculator
                .withdraw_single_token_type_exact_out(
                    results.owner_fee,
                    token_a_amount.into(),
                    token_b_amount.into(),
                    initial_supply.into(),
                    TradeDirection::BtoA,
                    RoundDirection::Floor,
                )
                .unwrap()
        } else {
            0
        };
        let fee_account =
            StateWithExtensions::<Account>::unpack(&accounts.pool_fee_account.data).unwrap();
        assert_eq!(
            fee_account.base.amount,
            to_u64(first_fee + second_fee).unwrap()
        );
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_valid_swap_curve_all_fees(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        // All fees
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 20;
        let host_fee_denominator = 100;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 10_000_000_000;
        let token_b_amount = 50_000_000_000;

        check_valid_swap_curve(
            fees.clone(),
            SwapTransferFees::default(),
            CurveType::ConstantProduct,
            Arc::new(ConstantProductCurve {}),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
        let token_b_price = 1;
        check_valid_swap_curve(
            fees.clone(),
            SwapTransferFees::default(),
            CurveType::ConstantPrice,
            Arc::new(ConstantPriceCurve { token_b_price }),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
        let token_b_offset = 10_000_000_000;
        check_valid_swap_curve(
            fees,
            SwapTransferFees::default(),
            CurveType::Offset,
            Arc::new(OffsetCurve { token_b_offset }),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_valid_swap_curve_trade_fee_only(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 0;
        let owner_trade_fee_denominator = 0;
        let owner_withdraw_fee_numerator = 0;
        let owner_withdraw_fee_denominator = 0;
        let host_fee_numerator = 0;
        let host_fee_denominator = 0;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 10_000_000_000;
        let token_b_amount = 50_000_000_000;

        check_valid_swap_curve(
            fees.clone(),
            SwapTransferFees::default(),
            CurveType::ConstantProduct,
            Arc::new(ConstantProductCurve {}),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
        let token_b_price = 10_000;
        check_valid_swap_curve(
            fees.clone(),
            SwapTransferFees::default(),
            CurveType::ConstantPrice,
            Arc::new(ConstantPriceCurve { token_b_price }),
            token_a_amount,
            token_b_amount / token_b_price,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
        let token_b_offset = 1;
        check_valid_swap_curve(
            fees,
            SwapTransferFees::default(),
            CurveType::Offset,
            Arc::new(OffsetCurve { token_b_offset }),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_valid_swap_with_fee_constraints(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let owner_key = Pubkey::new_unique();

        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 10;
        let host_fee_denominator = 100;

        let token_a_amount = 1_000_000;
        let token_b_amount = 5_000_000;

        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let curve = ConstantProductCurve {};
        let swap_curve = SwapCurve {
            curve_type: CurveType::ConstantProduct,
            calculator: Arc::new(curve),
        };

        let owner_key_str = owner_key.to_string();
        let valid_curve_types = &[CurveType::ConstantProduct];
        let constraints = Some(SwapConstraints {
            owner_key: Some(owner_key_str.as_ref()),
            valid_curve_types,
            fees: &fees,
        });
        let mut accounts = SwapAccountInfo::new(
            &owner_key,
            fees.clone(),
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        // initialize swap
        do_process_instruction_with_fee_constraints(
            initialize(
                &SWAP_PROGRAM_ID,
                &pool_token_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &accounts.pool_token_key,
                accounts.fees.clone(),
                accounts.swap_curve.clone(),
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut accounts.pool_mint_account,
                &mut accounts.pool_fee_account,
                &mut accounts.pool_token_account,
                &mut SolanaAccount::default(),
            ],
            &constraints,
        )
        .unwrap();

        let authority_key = accounts.authority_key;

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            &owner_key,
            &authority_key,
            token_a_amount,
            token_b_amount,
            0,
        );

        let amount_in = token_a_amount / 2;
        let minimum_amount_out = 0;

        // perform the swap
        do_process_instruction_with_fee_constraints(
            swap(
                &SWAP_PROGRAM_ID,
                &token_a_program_id,
                &token_b_program_id,
                &pool_token_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &accounts.authority_key,
                &token_a_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &token_b_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &accounts.token_a_mint_key,
                &accounts.token_b_mint_key,
                Some(&pool_key),
                Swap {
                    amount_in,
                    minimum_amount_out,
                },
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut token_a_account,
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut token_b_account,
                &mut accounts.pool_mint_account,
                &mut accounts.pool_fee_account,
                &mut accounts.token_a_mint_account,
                &mut accounts.token_b_mint_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut pool_account,
            ],
            &constraints,
        )
        .unwrap();

        // check that fees were taken in the host fee account
        let host_fee_account = StateWithExtensions::<Account>::unpack(&pool_account.data).unwrap();
        let owner_fee_account =
            StateWithExtensions::<Account>::unpack(&accounts.pool_fee_account.data).unwrap();
        let total_fee = owner_fee_account.base.amount * host_fee_denominator
            / (host_fee_denominator - host_fee_numerator);
        assert_eq!(
            total_fee,
            host_fee_account.base.amount + owner_fee_account.base.amount
        );
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_invalid_swap(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();
        let swapper_key = Pubkey::new_unique();
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 4;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 10;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 5;
        let host_fee_numerator = 9;
        let host_fee_denominator = 100;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 1000;
        let token_b_amount = 5000;
        let curve_type = CurveType::ConstantProduct;
        let swap_curve = SwapCurve {
            curve_type,
            calculator: Arc::new(ConstantProductCurve {}),
        };
        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        let initial_a = token_a_amount / 5;
        let initial_b = token_b_amount / 5;
        let minimum_token_b_amount = initial_b / 2;

        let swap_token_a_key = accounts.token_a_key;
        let swap_token_b_key = accounts.token_b_key;

        // swap not initialized
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(ProgramError::UninitializedAccount),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
        }

        accounts.initialize_swap().unwrap();

        // wrong swap account program id
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let old_swap_account = accounts.swap_account;
            let mut wrong_swap_account = old_swap_account.clone();
            wrong_swap_account.owner = pool_token_program_id;
            accounts.swap_account = wrong_swap_account;
            assert_eq!(
                Err(ProgramError::IncorrectProgramId),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
            accounts.swap_account = old_swap_account;
        }

        // wrong bump seed
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let old_authority = accounts.authority_key;
            let (bad_authority_key, _bump_seed) = Pubkey::find_program_address(
                &[&accounts.swap_key.to_bytes()[..]],
                &pool_token_program_id,
            );
            accounts.authority_key = bad_authority_key;
            assert_eq!(
                Err(SwapError::InvalidProgramAddress.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
            accounts.authority_key = old_authority;
        }

        // wrong token program id
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let wrong_program_id = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectTokenProgramId.into()),
                do_process_instruction(
                    swap(
                        &SWAP_PROGRAM_ID,
                        &wrong_program_id,
                        &wrong_program_id,
                        &wrong_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &token_a_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        None,
                        Swap {
                            amount_in: initial_a,
                            minimum_amount_out: minimum_token_b_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                ),
            );
        }

        // not enough token a to swap
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(TokenError::InsufficientFunds.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a * 2,
                    minimum_token_b_amount * 2,
                )
            );
        }

        // wrong swap token A / B accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let user_transfer_key = Pubkey::new_unique();
            assert_eq!(
                Err(SwapError::IncorrectSwapAccount.into()),
                do_process_instruction(
                    swap(
                        &SWAP_PROGRAM_ID,
                        &token_a_program_id,
                        &token_b_program_id,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_key,
                        &token_a_key,
                        &token_a_key,
                        &token_b_key,
                        &token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        None,
                        Swap {
                            amount_in: initial_a,
                            minimum_amount_out: minimum_token_b_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account.clone(),
                        &mut token_a_account,
                        &mut token_b_account.clone(),
                        &mut token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                ),
            );
        }

        // wrong user token A / B accounts
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(TokenError::MintMismatch.into()),
                accounts.swap(
                    &swapper_key,
                    &token_b_key,
                    &mut token_b_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_a_key,
                    &mut token_a_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
        }

        // swap from a to a
        {
            let (
                token_a_key,
                mut token_a_account,
                _token_b_key,
                _token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account.clone(),
                    &swap_token_a_key,
                    &swap_token_a_key,
                    &token_a_key,
                    &mut token_a_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
        }

        // incorrect mint provided
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let (pool_mint_key, pool_mint_account) = create_mint(
                &pool_token_program_id,
                &accounts.authority_key,
                None,
                None,
                &TransferFee::default(),
            );
            let old_pool_key = accounts.pool_mint_key;
            let old_pool_account = accounts.pool_mint_account;
            accounts.pool_mint_key = pool_mint_key;
            accounts.pool_mint_account = pool_mint_account;

            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );

            accounts.pool_mint_key = old_pool_key;
            accounts.pool_mint_account = old_pool_account;
        }

        // incorrect fee account provided
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                wrong_pool_key,
                wrong_pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let old_pool_fee_account = accounts.pool_fee_account;
            let old_pool_fee_key = accounts.pool_fee_key;
            accounts.pool_fee_account = wrong_pool_account;
            accounts.pool_fee_key = wrong_pool_key;
            assert_eq!(
                Err(SwapError::IncorrectFeeAccount.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
            accounts.pool_fee_account = old_pool_fee_account;
            accounts.pool_fee_key = old_pool_fee_key;
        }

        // no approval
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let user_transfer_key = Pubkey::new_unique();
            assert_eq!(
                Err(TokenError::OwnerMismatch.into()),
                do_process_instruction(
                    swap(
                        &SWAP_PROGRAM_ID,
                        &token_a_program_id,
                        &token_b_program_id,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &user_transfer_key,
                        &token_a_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        None,
                        Swap {
                            amount_in: initial_a,
                            minimum_amount_out: minimum_token_b_amount,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                    ],
                ),
            );
        }

        // output token value 0
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(SwapError::ZeroTradingTokens.into()),
                accounts.swap(
                    &swapper_key,
                    &token_b_key,
                    &mut token_b_account,
                    &swap_token_b_key,
                    &swap_token_a_key,
                    &token_a_key,
                    &mut token_a_account,
                    1,
                    1,
                )
            );
        }

        // slippage exceeded: minimum out amount too high
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(SwapError::ExceededSlippage.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount * 2,
                )
            );
        }

        // invalid input: can't use swap pool as user source / dest
        {
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            let mut swap_token_a_account = accounts.get_token_account(&swap_token_a_key).clone();
            let authority_key = accounts.authority_key;
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.swap(
                    &authority_key,
                    &swap_token_a_key,
                    &mut swap_token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &token_b_key,
                    &mut token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
            let mut swap_token_b_account = accounts.get_token_account(&swap_token_b_key).clone();
            assert_eq!(
                Err(SwapError::InvalidInput.into()),
                accounts.swap(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &swap_token_a_key,
                    &swap_token_b_key,
                    &swap_token_b_key,
                    &mut swap_token_b_account,
                    initial_a,
                    minimum_token_b_amount,
                )
            );
        }

        // still correct: constraint specified, no host fee account
        {
            let authority_key = accounts.authority_key;
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &authority_key, initial_a, initial_b, 0);
            let owner_key = swapper_key.to_string();
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let constraints = Some(SwapConstraints {
                owner_key: Some(owner_key.as_ref()),
                valid_curve_types: &[],
                fees: &fees,
            });
            do_process_instruction_with_fee_constraints(
                swap(
                    &SWAP_PROGRAM_ID,
                    &token_a_program_id,
                    &token_b_program_id,
                    &pool_token_program_id,
                    &accounts.swap_key,
                    &accounts.authority_key,
                    &accounts.authority_key,
                    &token_a_key,
                    &accounts.token_a_key,
                    &accounts.token_b_key,
                    &token_b_key,
                    &accounts.pool_mint_key,
                    &accounts.pool_fee_key,
                    &accounts.token_a_mint_key,
                    &accounts.token_b_mint_key,
                    None,
                    Swap {
                        amount_in: initial_a,
                        minimum_amount_out: minimum_token_b_amount,
                    },
                )
                .unwrap(),
                vec![
                    &mut accounts.swap_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut token_a_account,
                    &mut accounts.token_a_account,
                    &mut accounts.token_b_account,
                    &mut token_b_account,
                    &mut accounts.pool_mint_account,
                    &mut accounts.pool_fee_account,
                    &mut accounts.token_a_mint_account,
                    &mut accounts.token_b_mint_account,
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                    &mut SolanaAccount::default(),
                ],
                &constraints,
            )
            .unwrap();
        }

        // invalid mint for host fee account
        {
            let authority_key = accounts.authority_key;
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &authority_key, initial_a, initial_b, 0);
            let (
                bad_token_a_key,
                mut bad_token_a_account,
                _token_b_key,
                mut _token_b_account,
                _pool_key,
                _pool_account,
            ) = accounts.setup_token_accounts(&user_key, &authority_key, initial_a, initial_b, 0);
            let owner_key = swapper_key.to_string();
            let fees = Fees {
                trade_fee_numerator,
                trade_fee_denominator,
                owner_trade_fee_numerator,
                owner_trade_fee_denominator,
                owner_withdraw_fee_numerator,
                owner_withdraw_fee_denominator,
                host_fee_numerator,
                host_fee_denominator,
            };
            let constraints = Some(SwapConstraints {
                owner_key: Some(owner_key.as_ref()),
                valid_curve_types: &[],
                fees: &fees,
            });
            assert_eq!(
                Err(SwapError::IncorrectPoolMint.into()),
                do_process_instruction_with_fee_constraints(
                    swap(
                        &SWAP_PROGRAM_ID,
                        &token_a_program_id,
                        &token_b_program_id,
                        &pool_token_program_id,
                        &accounts.swap_key,
                        &accounts.authority_key,
                        &accounts.authority_key,
                        &token_a_key,
                        &accounts.token_a_key,
                        &accounts.token_b_key,
                        &token_b_key,
                        &accounts.pool_mint_key,
                        &accounts.pool_fee_key,
                        &accounts.token_a_mint_key,
                        &accounts.token_b_mint_key,
                        Some(&bad_token_a_key),
                        Swap {
                            amount_in: initial_a,
                            minimum_amount_out: 0,
                        },
                    )
                    .unwrap(),
                    vec![
                        &mut accounts.swap_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut token_a_account,
                        &mut accounts.token_a_account,
                        &mut accounts.token_b_account,
                        &mut token_b_account,
                        &mut accounts.pool_mint_account,
                        &mut accounts.pool_fee_account,
                        &mut accounts.token_a_mint_account,
                        &mut accounts.token_b_mint_account,
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut SolanaAccount::default(),
                        &mut bad_token_a_account,
                    ],
                    &constraints,
                ),
            );
        }
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_overdraw_offset_curve(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 10;
        let host_fee_denominator = 100;

        let token_a_amount = 1_000_000_000;
        let token_b_amount = 0;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_b_offset = 2_000_000;
        let swap_curve = SwapCurve {
            curve_type: CurveType::Offset,
            calculator: Arc::new(OffsetCurve { token_b_offset }),
        };
        let user_key = Pubkey::new_unique();
        let swapper_key = Pubkey::new_unique();

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let swap_token_a_key = accounts.token_a_key;
        let swap_token_b_key = accounts.token_b_key;
        let initial_a = 500_000;
        let initial_b = 1_000;

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            _pool_key,
            _pool_account,
        ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);

        // swap a to b way, fails, there's no liquidity
        let a_to_b_amount = initial_a;
        let minimum_token_b_amount = 0;

        assert_eq!(
            Err(SwapError::ZeroTradingTokens.into()),
            accounts.swap(
                &swapper_key,
                &token_a_key,
                &mut token_a_account,
                &swap_token_a_key,
                &swap_token_b_key,
                &token_b_key,
                &mut token_b_account,
                a_to_b_amount,
                minimum_token_b_amount,
            )
        );

        // swap b to a, succeeds at offset price
        let b_to_a_amount = initial_b;
        let minimum_token_a_amount = 0;
        accounts
            .swap(
                &swapper_key,
                &token_b_key,
                &mut token_b_account,
                &swap_token_b_key,
                &swap_token_a_key,
                &token_a_key,
                &mut token_a_account,
                b_to_a_amount,
                minimum_token_a_amount,
            )
            .unwrap();

        // try a to b again, succeeds due to new liquidity
        accounts
            .swap(
                &swapper_key,
                &token_a_key,
                &mut token_a_account,
                &swap_token_a_key,
                &swap_token_b_key,
                &token_b_key,
                &mut token_b_account,
                a_to_b_amount,
                minimum_token_b_amount,
            )
            .unwrap();

        // try a to b again, fails due to no more liquidity
        assert_eq!(
            Err(SwapError::ZeroTradingTokens.into()),
            accounts.swap(
                &swapper_key,
                &token_a_key,
                &mut token_a_account,
                &swap_token_a_key,
                &swap_token_b_key,
                &token_b_key,
                &mut token_b_account,
                a_to_b_amount,
                minimum_token_b_amount,
            )
        );

        // Try to deposit, fails because deposits are not allowed for offset
        // curve swaps
        {
            let initial_a = 100;
            let initial_b = 100;
            let pool_amount = 100;
            let (
                token_a_key,
                mut token_a_account,
                token_b_key,
                mut token_b_account,
                pool_key,
                mut pool_account,
            ) = accounts.setup_token_accounts(&user_key, &swapper_key, initial_a, initial_b, 0);
            assert_eq!(
                Err(SwapError::UnsupportedCurveOperation.into()),
                accounts.deposit_all_token_types(
                    &swapper_key,
                    &token_a_key,
                    &mut token_a_account,
                    &token_b_key,
                    &mut token_b_account,
                    &pool_key,
                    &mut pool_account,
                    pool_amount,
                    initial_a,
                    initial_b,
                )
            );
        }
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw_all_offset_curve(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 0;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 10;
        let host_fee_denominator = 100;

        let token_a_amount = 1_000_000_000;
        let token_b_amount = 10;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_b_offset = 2_000_000;
        let swap_curve = SwapCurve {
            curve_type: CurveType::Offset,
            calculator: Arc::new(OffsetCurve { token_b_offset }),
        };
        let total_pool = swap_curve.calculator.new_pool_supply();
        let user_key = Pubkey::new_unique();
        let withdrawer_key = Pubkey::new_unique();

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            _pool_key,
            _pool_account,
        ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, 0, 0, 0);

        let pool_key = accounts.pool_token_key;
        let mut pool_account = accounts.pool_token_account.clone();

        // WithdrawAllTokenTypes takes all tokens for A and B.
        // The curve's calculation for token B will say to transfer
        // `token_b_offset + token_b_amount`, but only `token_b_amount` will be
        // moved.
        accounts
            .withdraw_all_token_types(
                &user_key,
                &pool_key,
                &mut pool_account,
                &token_a_key,
                &mut token_a_account,
                &token_b_key,
                &mut token_b_account,
                total_pool.try_into().unwrap(),
                0,
                0,
            )
            .unwrap();

        let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
        assert_eq!(token_a.base.amount, token_a_amount);
        let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
        assert_eq!(token_b.base.amount, token_b_amount);
        let swap_token_a =
            StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
        assert_eq!(swap_token_a.base.amount, 0);
        let swap_token_b =
            StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
        assert_eq!(swap_token_b.base.amount, 0);
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw_all_constant_price_curve(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 0;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 10;
        let host_fee_denominator = 100;

        // initialize "unbalanced", so that withdrawing all will have some issues
        // A: 1_000_000_000
        // B: 2_000_000_000 (1_000 * 2_000_000)
        let swap_token_a_amount = 1_000_000_000;
        let swap_token_b_amount = 1_000;
        let token_b_price = 2_000_000;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let swap_curve = SwapCurve {
            curve_type: CurveType::ConstantPrice,
            calculator: Arc::new(ConstantPriceCurve { token_b_price }),
        };
        let total_pool = swap_curve.calculator.new_pool_supply();
        let user_key = Pubkey::new_unique();
        let withdrawer_key = Pubkey::new_unique();

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            swap_token_a_amount,
            swap_token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            _pool_key,
            _pool_account,
        ) = accounts.setup_token_accounts(&user_key, &withdrawer_key, 0, 0, 0);

        let pool_key = accounts.pool_token_key;
        let mut pool_account = accounts.pool_token_account.clone();

        // WithdrawAllTokenTypes will not take all token A and B, since their
        // ratio is unbalanced.  It will try to take 1_500_000_000 worth of
        // each token, which means 1_500_000_000 token A, and 750 token B.
        // With no slippage, this will leave 250 token B in the pool.
        assert_eq!(
            Err(SwapError::ExceededSlippage.into()),
            accounts.withdraw_all_token_types(
                &user_key,
                &pool_key,
                &mut pool_account,
                &token_a_key,
                &mut token_a_account,
                &token_b_key,
                &mut token_b_account,
                total_pool.try_into().unwrap(),
                swap_token_a_amount,
                swap_token_b_amount,
            )
        );

        accounts
            .withdraw_all_token_types(
                &user_key,
                &pool_key,
                &mut pool_account,
                &token_a_key,
                &mut token_a_account,
                &token_b_key,
                &mut token_b_account,
                total_pool.try_into().unwrap(),
                0,
                0,
            )
            .unwrap();

        let token_a = StateWithExtensions::<Account>::unpack(&token_a_account.data).unwrap();
        assert_eq!(token_a.base.amount, swap_token_a_amount);
        let token_b = StateWithExtensions::<Account>::unpack(&token_b_account.data).unwrap();
        assert_eq!(token_b.base.amount, 750);
        let swap_token_a =
            StateWithExtensions::<Account>::unpack(&accounts.token_a_account.data).unwrap();
        assert_eq!(swap_token_a.base.amount, 0);
        let swap_token_b =
            StateWithExtensions::<Account>::unpack(&accounts.token_b_account.data).unwrap();
        assert_eq!(swap_token_b.base.amount, 250);

        // deposit now, not enough to cover the tokens already in there
        let token_b_amount = 10;
        let token_a_amount = token_b_amount * token_b_price;
        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            &user_key,
            &withdrawer_key,
            token_a_amount,
            token_b_amount,
            0,
        );

        assert_eq!(
            Err(SwapError::ExceededSlippage.into()),
            accounts.deposit_all_token_types(
                &withdrawer_key,
                &token_a_key,
                &mut token_a_account,
                &token_b_key,
                &mut token_b_account,
                &pool_key,
                &mut pool_account,
                1, // doesn't matter
                token_a_amount,
                token_b_amount,
            )
        );

        // deposit enough tokens, success!
        let token_b_amount = 125;
        let token_a_amount = token_b_amount * token_b_price;
        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            &user_key,
            &withdrawer_key,
            token_a_amount,
            token_b_amount,
            0,
        );

        accounts
            .deposit_all_token_types(
                &withdrawer_key,
                &token_a_key,
                &mut token_a_account,
                &token_b_key,
                &mut token_b_account,
                &pool_key,
                &mut pool_account,
                1, // doesn't matter
                token_a_amount,
                token_b_amount,
            )
            .unwrap();
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_deposits_allowed_single_token(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 0;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 10;
        let host_fee_denominator = 100;

        let token_a_amount = 1_000_000;
        let token_b_amount = 0;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_b_offset = 2_000_000;
        let swap_curve = SwapCurve {
            curve_type: CurveType::Offset,
            calculator: Arc::new(OffsetCurve { token_b_offset }),
        };
        let creator_key = Pubkey::new_unique();
        let depositor_key = Pubkey::new_unique();

        let mut accounts = SwapAccountInfo::new(
            &creator_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let initial_a = 1_000_000;
        let initial_b = 2_000_000;
        let (
            _depositor_token_a_key,
            _depositor_token_a_account,
            depositor_token_b_key,
            mut depositor_token_b_account,
            depositor_pool_key,
            mut depositor_pool_account,
        ) = accounts.setup_token_accounts(&creator_key, &depositor_key, initial_a, initial_b, 0);

        assert_eq!(
            Err(SwapError::UnsupportedCurveOperation.into()),
            accounts.deposit_single_token_type_exact_amount_in(
                &depositor_key,
                &depositor_token_b_key,
                &mut depositor_token_b_account,
                &depositor_pool_key,
                &mut depositor_pool_account,
                initial_b,
                0,
            )
        );
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw_with_invalid_fee_account(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();

        let fees = Fees {
            trade_fee_numerator: 1,
            trade_fee_denominator: 2,
            owner_trade_fee_numerator: 1,
            owner_trade_fee_denominator: 10,
            owner_withdraw_fee_numerator: 1,
            owner_withdraw_fee_denominator: 5,
            host_fee_numerator: 7,
            host_fee_denominator: 100,
        };

        let token_a_amount = 1000;
        let token_b_amount = 2000;
        let swap_curve = SwapCurve {
            curve_type: CurveType::ConstantProduct,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let withdrawer_key = Pubkey::new_unique();
        let initial_a = token_a_amount / 10;
        let initial_b = token_b_amount / 10;
        let initial_pool = swap_curve.calculator.new_pool_supply() / 10;
        let withdraw_amount = initial_pool / 4;
        let minimum_token_a_amount = initial_a / 40;
        let minimum_token_b_amount = initial_b / 40;

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            &user_key,
            &withdrawer_key,
            initial_a,
            initial_b,
            initial_pool.try_into().unwrap(),
        );

        let destination_key = Pubkey::new_unique();
        let mut destination = SolanaAccount::new(
            account_minimum_balance(),
            Account::get_packed_len(),
            &withdrawer_key,
        );

        do_process_instruction(
            close_account(
                &pool_token_program_id,
                &accounts.pool_fee_key,
                &destination_key,
                &user_key,
                &[],
            )
            .unwrap(),
            vec![
                &mut accounts.pool_fee_account,
                &mut destination,
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();

        let user_transfer_authority_key = Pubkey::new_unique();
        let pool_token_amount = withdraw_amount.try_into().unwrap();

        do_process_instruction(
            approve(
                &pool_token_program_id,
                &pool_key,
                &user_transfer_authority_key,
                &withdrawer_key,
                &[],
                pool_token_amount,
            )
            .unwrap(),
            vec![
                &mut pool_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();

        do_process_instruction(
            withdraw_all_token_types(
                &SWAP_PROGRAM_ID,
                &pool_token_program_id,
                &token_a_program_id,
                &token_b_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &user_transfer_authority_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &pool_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &token_a_key,
                &token_b_key,
                &accounts.token_a_mint_key,
                &accounts.token_b_mint_key,
                WithdrawAllTokenTypes {
                    pool_token_amount,
                    minimum_token_a_amount,
                    minimum_token_b_amount,
                },
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut accounts.pool_mint_account,
                &mut pool_account,
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut token_a_account,
                &mut token_b_account,
                &mut accounts.pool_fee_account,
                &mut accounts.token_a_mint_account,
                &mut accounts.token_b_mint_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_withdraw_one_exact_out_with_invalid_fee_account(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let user_key = Pubkey::new_unique();

        let fees = Fees {
            trade_fee_numerator: 1,
            trade_fee_denominator: 2,
            owner_trade_fee_numerator: 1,
            owner_trade_fee_denominator: 10,
            owner_withdraw_fee_numerator: 1,
            owner_withdraw_fee_denominator: 5,
            host_fee_numerator: 7,
            host_fee_denominator: 100,
        };

        let token_a_amount = 1000;
        let token_b_amount = 2000;
        let swap_curve = SwapCurve {
            curve_type: CurveType::ConstantProduct,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let withdrawer_key = Pubkey::new_unique();
        let initial_a = token_a_amount / 10;
        let initial_b = token_b_amount / 10;
        let initial_pool = swap_curve.calculator.new_pool_supply() / 10;
        let maximum_pool_token_amount = to_u64(initial_pool / 4).unwrap();
        let destination_a_amount = initial_a / 40;

        let mut accounts = SwapAccountInfo::new(
            &user_key,
            fees,
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        accounts.initialize_swap().unwrap();

        let (
            token_a_key,
            mut token_a_account,
            _token_b_key,
            _token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            &user_key,
            &withdrawer_key,
            initial_a,
            initial_b,
            initial_pool.try_into().unwrap(),
        );

        let destination_key = Pubkey::new_unique();
        let mut destination = SolanaAccount::new(
            account_minimum_balance(),
            Account::get_packed_len(),
            &withdrawer_key,
        );

        do_process_instruction(
            close_account(
                &pool_token_program_id,
                &accounts.pool_fee_key,
                &destination_key,
                &user_key,
                &[],
            )
            .unwrap(),
            vec![
                &mut accounts.pool_fee_account,
                &mut destination,
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();

        let user_transfer_authority_key = Pubkey::new_unique();

        do_process_instruction(
            approve(
                &pool_token_program_id,
                &pool_key,
                &user_transfer_authority_key,
                &withdrawer_key,
                &[],
                maximum_pool_token_amount,
            )
            .unwrap(),
            vec![
                &mut pool_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();

        do_process_instruction(
            withdraw_single_token_type_exact_amount_out(
                &SWAP_PROGRAM_ID,
                &pool_token_program_id,
                &token_a_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &user_transfer_authority_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &pool_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &token_a_key,
                &accounts.token_a_mint_key,
                WithdrawSingleTokenTypeExactAmountOut {
                    destination_token_amount: destination_a_amount,
                    maximum_pool_token_amount,
                },
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut accounts.pool_mint_account,
                &mut pool_account,
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut token_a_account,
                &mut accounts.pool_fee_account,
                &mut accounts.token_a_mint_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();
    }

    #[test_case(spl_token::id(), spl_token::id(), spl_token::id(); "all-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_valid_swap_with_invalid_fee_account(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        let owner_key = &Pubkey::new_unique();

        let token_a_amount = 1_000_000;
        let token_b_amount = 5_000_000;

        let fees = Fees {
            trade_fee_numerator: 1,
            trade_fee_denominator: 10,
            owner_trade_fee_numerator: 1,
            owner_trade_fee_denominator: 30,
            owner_withdraw_fee_numerator: 1,
            owner_withdraw_fee_denominator: 30,
            host_fee_numerator: 10,
            host_fee_denominator: 100,
        };

        let swap_curve = SwapCurve {
            curve_type: CurveType::ConstantProduct,
            calculator: Arc::new(ConstantProductCurve {}),
        };

        let owner_key_str = owner_key.to_string();
        let constraints = Some(SwapConstraints {
            owner_key: Some(owner_key_str.as_ref()),
            valid_curve_types: &[CurveType::ConstantProduct],
            fees: &fees,
        });
        let mut accounts = SwapAccountInfo::new(
            owner_key,
            fees.clone(),
            SwapTransferFees::default(),
            swap_curve,
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );

        do_process_instruction_with_fee_constraints(
            initialize(
                &SWAP_PROGRAM_ID,
                &pool_token_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &accounts.pool_token_key,
                accounts.fees.clone(),
                accounts.swap_curve.clone(),
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut accounts.pool_mint_account,
                &mut accounts.pool_fee_account,
                &mut accounts.pool_token_account,
                &mut SolanaAccount::default(),
            ],
            &constraints,
        )
        .unwrap();

        let authority_key = accounts.authority_key;

        let (
            token_a_key,
            mut token_a_account,
            token_b_key,
            mut token_b_account,
            pool_key,
            mut pool_account,
        ) = accounts.setup_token_accounts(
            owner_key,
            &authority_key,
            token_a_amount,
            token_b_amount,
            0,
        );

        let destination_key = Pubkey::new_unique();
        let mut destination = SolanaAccount::new(
            account_minimum_balance(),
            Account::get_packed_len(),
            owner_key,
        );

        do_process_instruction(
            close_account(
                &pool_token_program_id,
                &accounts.pool_fee_key,
                &destination_key,
                owner_key,
                &[],
            )
            .unwrap(),
            vec![
                &mut accounts.pool_fee_account,
                &mut destination,
                &mut SolanaAccount::default(),
            ],
        )
        .unwrap();

        do_process_instruction_with_fee_constraints(
            swap(
                &SWAP_PROGRAM_ID,
                &token_a_program_id,
                &token_b_program_id,
                &pool_token_program_id,
                &accounts.swap_key,
                &accounts.authority_key,
                &accounts.authority_key,
                &token_a_key,
                &accounts.token_a_key,
                &accounts.token_b_key,
                &token_b_key,
                &accounts.pool_mint_key,
                &accounts.pool_fee_key,
                &accounts.token_a_mint_key,
                &accounts.token_b_mint_key,
                Some(&pool_key),
                Swap {
                    amount_in: token_a_amount / 2,
                    minimum_amount_out: 0,
                },
            )
            .unwrap(),
            vec![
                &mut accounts.swap_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut token_a_account,
                &mut accounts.token_a_account,
                &mut accounts.token_b_account,
                &mut token_b_account,
                &mut accounts.pool_mint_account,
                &mut accounts.pool_fee_account,
                &mut accounts.token_a_mint_account,
                &mut accounts.token_b_mint_account,
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut SolanaAccount::default(),
                &mut pool_account,
            ],
            &constraints,
        )
        .unwrap();
    }

    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token_2022::id(); "all-token-2022")]
    #[test_case(spl_token::id(), spl_token_2022::id(), spl_token_2022::id(); "mixed-pool-token")]
    #[test_case(spl_token_2022::id(), spl_token_2022::id(), spl_token::id(); "mixed-pool-token-2022")]
    fn test_swap_curve_with_transfer_fees(
        pool_token_program_id: Pubkey,
        token_a_program_id: Pubkey,
        token_b_program_id: Pubkey,
    ) {
        // All fees
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 10;
        let owner_trade_fee_numerator = 1;
        let owner_trade_fee_denominator = 30;
        let owner_withdraw_fee_numerator = 1;
        let owner_withdraw_fee_denominator = 30;
        let host_fee_numerator = 20;
        let host_fee_denominator = 100;
        let fees = Fees {
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        };

        let token_a_amount = 10_000_000_000;
        let token_b_amount = 50_000_000_000;

        check_valid_swap_curve(
            fees,
            SwapTransferFees {
                pool_token: TransferFee::default(),
                token_a: TransferFee {
                    epoch: 0.into(),
                    transfer_fee_basis_points: 100.into(),
                    maximum_fee: 1_000_000_000.into(),
                },
                token_b: TransferFee::default(),
            },
            CurveType::ConstantProduct,
            Arc::new(ConstantProductCurve {}),
            token_a_amount,
            token_b_amount,
            &pool_token_program_id,
            &token_a_program_id,
            &token_b_program_id,
        );
    }
}
