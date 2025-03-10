//! State transition types

use {
    crate::{
        curve::{base::SwapCurve, fees::Fees},
        error::SwapError,
    },
    arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs},
    enum_dispatch::enum_dispatch,
    solana_program::{
        account_info::AccountInfo,
        msg,
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
        pubkey::Pubkey,
    },
    spl_token_2022::{
        extension::StateWithExtensions,
        state::{Account, AccountState},
    },
    std::sync::Arc,
};

/// Trait representing access to program state across all versions
#[enum_dispatch]
pub trait SwapState {
    /// Is the swap initialized, with data written to it
    fn is_initialized(&self) -> bool;
    /// Bump seed used to generate the program address / authority
    fn bump_seed(&self) -> u8;
    /// Token program ID associated with the swap
    fn token_program_id(&self) -> &Pubkey;
    /// Address of token A liquidity account
    fn token_a_account(&self) -> &Pubkey;
    /// Address of token B liquidity account
    fn token_b_account(&self) -> &Pubkey;
    /// Address of pool token mint
    fn pool_mint(&self) -> &Pubkey;

    /// Address of token A mint
    fn token_a_mint(&self) -> &Pubkey;
    /// Address of token B mint
    fn token_b_mint(&self) -> &Pubkey;

    /// Address of pool fee account
    fn pool_fee_account(&self) -> &Pubkey;
    /// Check if the pool fee info is a valid token program account
    /// capable of receiving tokens from the mint.
    fn check_pool_fee_info(&self, pool_fee_info: &AccountInfo) -> Result<(), ProgramError>;

    /// Fees associated with swap
    fn fees(&self) -> &Fees;
    /// Curve associated with swap
    fn swap_curve(&self) -> &SwapCurve;
}

/// All versions of SwapState
#[enum_dispatch(SwapState)]
pub enum SwapVersion {
    /// Latest version, used for all new swaps
    SwapV1,
}

/// SwapVersion does not implement program_pack::Pack because there are size
/// checks on pack and unpack that would break backwards compatibility, so
/// special implementations are provided here
impl SwapVersion {
    /// Size of the latest version of the SwapState
    pub const LATEST_LEN: usize = 1 + SwapV1::LEN; // add one for the version enum

    /// Pack a swap into a byte array, based on its version
    /// 方法的核心作用是：
    // 	•	将 SwapV1 结构体转换为可存储的 u8 数组。
    // 	•	支持未来扩展（如果有新版本 SwapV2，可以通过 dst[0] 识别并处理不同版本）。

    // 在 Solana 智能合约中，账户的数据存储方式通常是 u8 数组，所以 pack 方法就是 一个自定义的序列化逻辑。
    /// 	1.	dst[0] = 1;
    // •	标记 Swap 版本号，用于未来升级兼容性（如果以后有 SwapV2、SwapV3，可以用 dst[0] 区分）。
    // 2.	SwapV1::pack(swap_info, &mut dst[1..])
    // •	调用 SwapV1::pack 方法，将 swap_info（SwapV1 结构体）转换为 byte array，并存入 dst[1..]。
    pub fn pack(src: Self, dst: &mut [u8]) -> Result<(), ProgramError> {
        match src {
            Self::SwapV1(swap_info) => {
                dst[0] = 1;
                SwapV1::pack(swap_info, &mut dst[1..])
            }
        }
    }

    /// Unpack the swap account based on its version, returning the result as a
    /// SwapState trait object
    pub fn unpack(input: &[u8]) -> Result<Arc<dyn SwapState>, ProgramError> {
        let (&version, rest) = input
            .split_first()
            .ok_or(ProgramError::InvalidAccountData)?;
        match version {
            1 => Ok(Arc::new(SwapV1::unpack(rest)?)),
            _ => Err(ProgramError::UninitializedAccount),
        }
    }

    /// Special check to be done before any instruction processing, works for
    /// all versions
    pub fn is_initialized(input: &[u8]) -> bool {
        match Self::unpack(input) {
            Ok(swap) => swap.is_initialized(),
            Err(_) => false,
        }
    }
}

/// Program states.
/// SwapV1 结构体定义了 Solana AMM 交易池的核心状态：
// 	1.	流动性池状态（is_initialized）
// 	2.	PDA 计算参数（bump_seed）
// 	3.	交易代币信息（token_a、token_b、token_a_mint、token_b_mint）
// 	4.	流动性池代币（LP Token）管理（pool_mint）
// 	5.	费用收取账户（pool_fee_account）
// 	6.	交易费用结构（fees）
// 	7.	流动性池交易曲线（swap_curve）
// 这个结构体在 Solana Token Swap 过程中起到了 存储和管理整个 AMM 交易池的作用，并确保 交易安全性和一致性。
#[repr(C)]
#[derive(Debug, Default, PartialEq)]
pub struct SwapV1 {
    /// Initialized state.
    pub is_initialized: bool,
    /// Bump seed used in program address.
    /// The program address is created deterministically with the bump seed,
    /// swap program id, and swap account pubkey.  This program address has
    /// authority over the swap's token A account, token B account, and pool
    /// token mint.
    pub bump_seed: u8,

    /// Program ID of the tokens being exchanged.
    pub token_program_id: Pubkey,

    /// Token A
    pub token_a: Pubkey,
    /// Token B
    pub token_b: Pubkey,

    /// Pool tokens are issued when A or B tokens are deposited.
    /// Pool tokens can be withdrawn back to the original A or B token.
    pub pool_mint: Pubkey,

    /// Mint information for token A
    pub token_a_mint: Pubkey,
    /// Mint information for token B
    pub token_b_mint: Pubkey,

    /// Pool token account to receive trading and / or withdrawal fees
    pub pool_fee_account: Pubkey,

    /// All fee information
    pub fees: Fees,

    /// Swap curve parameters, to be unpacked and used by the SwapCurve, which
    /// calculates swaps, deposits, and withdrawals
    pub swap_curve: SwapCurve,
}

impl SwapState for SwapV1 {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }

    fn bump_seed(&self) -> u8 {
        self.bump_seed
    }

    fn token_program_id(&self) -> &Pubkey {
        &self.token_program_id
    }

    fn token_a_account(&self) -> &Pubkey {
        &self.token_a
    }

    fn token_b_account(&self) -> &Pubkey {
        &self.token_b
    }

    fn pool_mint(&self) -> &Pubkey {
        &self.pool_mint
    }

    fn token_a_mint(&self) -> &Pubkey {
        &self.token_a_mint
    }

    fn token_b_mint(&self) -> &Pubkey {
        &self.token_b_mint
    }

    fn pool_fee_account(&self) -> &Pubkey {
        &self.pool_fee_account
    }

    fn check_pool_fee_info(&self, pool_fee_info: &AccountInfo) -> Result<(), ProgramError> {
        let data = &pool_fee_info.data.borrow();
        let token_account =
            StateWithExtensions::<Account>::unpack(data).map_err(|err| match err {
                ProgramError::InvalidAccountData | ProgramError::UninitializedAccount => {
                    SwapError::InvalidFeeAccount.into()
                }
                _ => err,
            })?;
        if pool_fee_info.owner != &self.token_program_id
            || token_account.base.state != AccountState::Initialized
            || token_account.base.mint != self.pool_mint
        {
            msg!("Pool fee account is not owned by token program, is not initialized, or does not match stake pool's mint");
            return Err(SwapError::InvalidFeeAccount.into());
        }
        Ok(())
    }

    fn fees(&self) -> &Fees {
        &self.fees
    }

    fn swap_curve(&self) -> &SwapCurve {
        &self.swap_curve
    }
}

impl Sealed for SwapV1 {}
impl IsInitialized for SwapV1 {
    fn is_initialized(&self) -> bool {
        self.is_initialized
    }
}

impl Pack for SwapV1 {
    const LEN: usize = 323;

    fn pack_into_slice(&self, output: &mut [u8]) {
        let output = array_mut_ref![output, 0, 323];
        let (
            is_initialized,
            bump_seed,
            token_program_id,
            token_a,
            token_b,
            pool_mint,
            token_a_mint,
            token_b_mint,
            pool_fee_account,
            fees,
            swap_curve,
        ) = mut_array_refs![output, 1, 1, 32, 32, 32, 32, 32, 32, 32, 64, 33];
        is_initialized[0] = self.is_initialized as u8;
        bump_seed[0] = self.bump_seed;
        token_program_id.copy_from_slice(self.token_program_id.as_ref());
        token_a.copy_from_slice(self.token_a.as_ref());
        token_b.copy_from_slice(self.token_b.as_ref());
        pool_mint.copy_from_slice(self.pool_mint.as_ref());
        token_a_mint.copy_from_slice(self.token_a_mint.as_ref());
        token_b_mint.copy_from_slice(self.token_b_mint.as_ref());
        pool_fee_account.copy_from_slice(self.pool_fee_account.as_ref());
        self.fees.pack_into_slice(&mut fees[..]);
        self.swap_curve.pack_into_slice(&mut swap_curve[..]);
    }

    /// Unpacks a byte buffer into a [SwapV1](struct.SwapV1.html).
    fn unpack_from_slice(input: &[u8]) -> Result<Self, ProgramError> {
        let input = array_ref![input, 0, 323];
        #[allow(clippy::ptr_offset_with_cast)]
        let (
            is_initialized,
            bump_seed,
            token_program_id,
            token_a,
            token_b,
            pool_mint,
            token_a_mint,
            token_b_mint,
            pool_fee_account,
            fees,
            swap_curve,
        ) = array_refs![input, 1, 1, 32, 32, 32, 32, 32, 32, 32, 64, 33];
        Ok(Self {
            is_initialized: match is_initialized {
                [0] => false,
                [1] => true,
                _ => return Err(ProgramError::InvalidAccountData),
            },
            bump_seed: bump_seed[0],
            token_program_id: Pubkey::new_from_array(*token_program_id),
            token_a: Pubkey::new_from_array(*token_a),
            token_b: Pubkey::new_from_array(*token_b),
            pool_mint: Pubkey::new_from_array(*pool_mint),
            token_a_mint: Pubkey::new_from_array(*token_a_mint),
            token_b_mint: Pubkey::new_from_array(*token_b_mint),
            pool_fee_account: Pubkey::new_from_array(*pool_fee_account),
            fees: Fees::unpack_from_slice(fees)?,
            swap_curve: SwapCurve::unpack_from_slice(swap_curve)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use {super::*, crate::curve::offset::OffsetCurve, std::convert::TryInto};

    const TEST_FEES: Fees = Fees {
        trade_fee_numerator: 1,
        trade_fee_denominator: 4,
        owner_trade_fee_numerator: 3,
        owner_trade_fee_denominator: 10,
        owner_withdraw_fee_numerator: 2,
        owner_withdraw_fee_denominator: 7,
        host_fee_numerator: 5,
        host_fee_denominator: 20,
    };

    const TEST_BUMP_SEED: u8 = 255;
    const TEST_TOKEN_PROGRAM_ID: Pubkey = Pubkey::new_from_array([1u8; 32]);
    const TEST_TOKEN_A: Pubkey = Pubkey::new_from_array([2u8; 32]);
    const TEST_TOKEN_B: Pubkey = Pubkey::new_from_array([3u8; 32]);
    const TEST_POOL_MINT: Pubkey = Pubkey::new_from_array([4u8; 32]);
    const TEST_TOKEN_A_MINT: Pubkey = Pubkey::new_from_array([5u8; 32]);
    const TEST_TOKEN_B_MINT: Pubkey = Pubkey::new_from_array([6u8; 32]);
    const TEST_POOL_FEE_ACCOUNT: Pubkey = Pubkey::new_from_array([7u8; 32]);

    const TEST_CURVE_TYPE: u8 = 2;
    const TEST_TOKEN_B_OFFSET: u64 = 1_000_000_000;
    const TEST_CURVE: OffsetCurve = OffsetCurve {
        token_b_offset: TEST_TOKEN_B_OFFSET,
    };

    #[test]
    fn swap_version_pack() {
        let curve_type = TEST_CURVE_TYPE.try_into().unwrap();
        let calculator = Arc::new(TEST_CURVE);
        let swap_curve = SwapCurve {
            curve_type,
            calculator,
        };
        let swap_info = SwapVersion::SwapV1(SwapV1 {
            is_initialized: true,
            bump_seed: TEST_BUMP_SEED,
            token_program_id: TEST_TOKEN_PROGRAM_ID,
            token_a: TEST_TOKEN_A,
            token_b: TEST_TOKEN_B,
            pool_mint: TEST_POOL_MINT,
            token_a_mint: TEST_TOKEN_A_MINT,
            token_b_mint: TEST_TOKEN_B_MINT,
            pool_fee_account: TEST_POOL_FEE_ACCOUNT,
            fees: TEST_FEES,
            swap_curve: swap_curve.clone(),
        });

        let mut packed = [0u8; SwapVersion::LATEST_LEN];
        SwapVersion::pack(swap_info, &mut packed).unwrap();
        let unpacked = SwapVersion::unpack(&packed).unwrap();

        assert!(unpacked.is_initialized());
        assert_eq!(unpacked.bump_seed(), TEST_BUMP_SEED);
        assert_eq!(*unpacked.token_program_id(), TEST_TOKEN_PROGRAM_ID);
        assert_eq!(*unpacked.token_a_account(), TEST_TOKEN_A);
        assert_eq!(*unpacked.token_b_account(), TEST_TOKEN_B);
        assert_eq!(*unpacked.pool_mint(), TEST_POOL_MINT);
        assert_eq!(*unpacked.token_a_mint(), TEST_TOKEN_A_MINT);
        assert_eq!(*unpacked.token_b_mint(), TEST_TOKEN_B_MINT);
        assert_eq!(*unpacked.pool_fee_account(), TEST_POOL_FEE_ACCOUNT);
        assert_eq!(*unpacked.fees(), TEST_FEES);
        assert_eq!(*unpacked.swap_curve(), swap_curve);
    }

    #[test]
    fn swap_v1_pack() {
        let curve_type = TEST_CURVE_TYPE.try_into().unwrap();
        let calculator = Arc::new(TEST_CURVE);
        let swap_curve = SwapCurve {
            curve_type,
            calculator,
        };
        let swap_info = SwapV1 {
            is_initialized: true,
            bump_seed: TEST_BUMP_SEED,
            token_program_id: TEST_TOKEN_PROGRAM_ID,
            token_a: TEST_TOKEN_A,
            token_b: TEST_TOKEN_B,
            pool_mint: TEST_POOL_MINT,
            token_a_mint: TEST_TOKEN_A_MINT,
            token_b_mint: TEST_TOKEN_B_MINT,
            pool_fee_account: TEST_POOL_FEE_ACCOUNT,
            fees: TEST_FEES,
            swap_curve,
        };

        let mut packed = [0u8; SwapV1::LEN];
        SwapV1::pack_into_slice(&swap_info, &mut packed);
        let unpacked = SwapV1::unpack(&packed).unwrap();
        assert_eq!(swap_info, unpacked);

        let mut packed = vec![1u8, TEST_BUMP_SEED];
        packed.extend_from_slice(&TEST_TOKEN_PROGRAM_ID.to_bytes());
        packed.extend_from_slice(&TEST_TOKEN_A.to_bytes());
        packed.extend_from_slice(&TEST_TOKEN_B.to_bytes());
        packed.extend_from_slice(&TEST_POOL_MINT.to_bytes());
        packed.extend_from_slice(&TEST_TOKEN_A_MINT.to_bytes());
        packed.extend_from_slice(&TEST_TOKEN_B_MINT.to_bytes());
        packed.extend_from_slice(&TEST_POOL_FEE_ACCOUNT.to_bytes());
        packed.extend_from_slice(&TEST_FEES.trade_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.trade_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.owner_trade_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.owner_trade_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.owner_withdraw_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.owner_withdraw_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.host_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&TEST_FEES.host_fee_denominator.to_le_bytes());
        packed.push(TEST_CURVE_TYPE);
        packed.extend_from_slice(&TEST_TOKEN_B_OFFSET.to_le_bytes());
        packed.extend_from_slice(&[0u8; 24]);
        let unpacked = SwapV1::unpack(&packed).unwrap();
        assert_eq!(swap_info, unpacked);

        let packed = [0u8; SwapV1::LEN];
        let swap_info: SwapV1 = Default::default();
        let unpack_unchecked = SwapV1::unpack_unchecked(&packed).unwrap();
        assert_eq!(unpack_unchecked, swap_info);
        let err = SwapV1::unpack(&packed).unwrap_err();
        assert_eq!(err, ProgramError::UninitializedAccount);
    }
}
