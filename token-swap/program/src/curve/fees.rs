//! All fee information, to be used for validation currently

use {
    crate::error::SwapError,
    arrayref::{array_mut_ref, array_ref, array_refs, mut_array_refs},
    solana_program::{
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
    },
};

/// Encapsulates all fee information and calculations for swap operations
/// 封装 Solana Token Swap 程序中的所有手续费信息及计算逻辑，主要涉及 交易费（trade fees）、所有者费用（owner fees）、主机费用（host fees）。
/// Fees 结构体用于 管理 Token Swap 交易的手续费，包括：
// 	1.	交易费用（Trade Fees）：提高流动性池 LP 代币价值。
// 	2.	合约所有者交易费用（Owner Trade Fees）：奖励合约所有者。
// 	3.	合约所有者提款费用（Owner Withdraw Fees）：用户提取流动性时，合约所有者额外收取 LP 代币。
// 	4.	主机费用（Host Fees）：从 owner_trade_fee 分配一部分给提供 Swap 服务的 DApp/平台。
// 通过这种多层收费模式，既能 激励流动性提供者，又能 保障合约所有者和 DApp 生态的利益。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fees {
    // •	交易费用（Trade Fees）：
    // •	在 Swap 交易时，收取 trade_fee_numerator / trade_fee_denominator 作为交易费用。
    // •	这些 Token 不会直接给流动性提供者，而是存留在池子里，使得 LP 代币的价值增加。
    // •	例如，如果 trade_fee_numerator = 3，trade_fee_denominator = 1000，表示 0.3% 交易费。
    /// Trade fees are extra token amounts that are held inside the token
    /// accounts during a trade, making the value of liquidity tokens rise.
    /// Trade fee numerator
    pub trade_fee_numerator: u64,
    /// Trade fee denominator
    pub trade_fee_denominator: u64,

    // 合约所有者费用（Owner Trade Fees）：
    // •	在 Swap 交易时，除了 trade_fee 之外，还会额外收取一部分交易费用，并转换为流动性代币（LP Token）奖励给合约所有者。
    // •	计算方式类似 trade_fee，即 owner_trade_fee_numerator / owner_trade_fee_denominator。
    // •	例如，如果 owner_trade_fee_numerator = 1，owner_trade_fee_denominator = 1000，表示 0.1% 交易费归合约所有者。
    /// Owner trading fees are extra token amounts that are held inside the
    /// token accounts during a trade, with the equivalent in pool tokens
    /// minted to the owner of the program.
    /// Owner trade fee numerator
    pub owner_trade_fee_numerator: u64,
    /// Owner trade fee denominator
    pub owner_trade_fee_denominator: u64,

    // 合约所有者提款费用（Owner Withdraw Fees）：
    // •	当流动性提供者从池子中**提取流动性（取回 TokenA 和 TokenB）**时，所有者会额外收取一部分 LP 代币。
    // •	计算方式：owner_withdraw_fee_numerator / owner_withdraw_fee_denominator。
    // •	例如，owner_withdraw_fee_numerator = 1，owner_withdraw_fee_denominator = 1000，表示 0.1% 提款手续费归合约所有者。
    /// Owner withdraw fees are extra liquidity pool token amounts that are
    /// sent to the owner on every withdrawal.
    /// Owner withdraw fee numerator
    pub owner_withdraw_fee_numerator: u64,
    /// Owner withdraw fee denominator
    pub owner_withdraw_fee_denominator: u64,

    // 主机费用（Host Fees）：
    // •	在 owner_trade_fee 中，会额外分配一部分给 调用此 Swap 交易的 DApp 或合作平台。
    // •	计算方式：host_fee_numerator / host_fee_denominator，但这部分费用是从 owner_trade_fee 中扣除，而不是额外收费。
    // •	例如：
    // •	owner_trade_fee = 0.1%
    // •	host_fee = 20%（从 owner_trade_fee 分配）
    // •	计算：如果用户交易了 1000 USDT：
    // •	交易费 = 3 USDT（假设 trade_fee = 0.3%）
    // •	所有者交易费 = 1 USDT（假设 owner_trade_fee = 0.1%）
    // •	主机费用 = 0.2 USDT（1 USDT * 20%）
    /// Host fees are a proportion of the owner trading fees, sent to an
    /// extra account provided during the trade.
    /// Host trading fee numerator
    pub host_fee_numerator: u64,
    /// Host trading fee denominator
    pub host_fee_denominator: u64,
}

/// Helper function for calculating swap fee
/// 	1.	如果手续费分子或代币金额为 0，则没有手续费，返回 Some(0)。
// 2.	使用 checked_mul 和 checked_div 来计算手续费，确保计算过程中没有溢出或除零错误。
// 3.	如果计算出的手续费为 0，返回至少 1 个代币的手续费（作为最低手续费）。
// 4.	如果计算出的手续费大于 0，则返回实际计算的手续费。
pub fn calculate_fee(
    token_amount: u128,
    fee_numerator: u128,
    fee_denominator: u128,
) -> Option<u128> {
    if fee_numerator == 0 || token_amount == 0 {
        Some(0)
    } else {
        let fee = token_amount
            .checked_mul(fee_numerator)?
            .checked_div(fee_denominator)?;
        if fee == 0 {
            Some(1) // minimum fee of one token
        } else {
            Some(fee)
        }
    }
}

fn ceil_div(dividend: u128, divisor: u128) -> Option<u128> {
    dividend
        .checked_add(divisor)?
        .checked_sub(1)?
        .checked_div(divisor)
}

fn pre_fee_amount(
    post_fee_amount: u128,
    fee_numerator: u128,
    fee_denominator: u128,
) -> Option<u128> {
    if fee_numerator == 0 || fee_denominator == 0 {
        Some(post_fee_amount)
    } else if fee_numerator == fee_denominator || post_fee_amount == 0 {
        Some(0)
    } else {
        let numerator = post_fee_amount.checked_mul(fee_denominator)?;
        let denominator = fee_denominator.checked_sub(fee_numerator)?;
        ceil_div(numerator, denominator)
    }
}

fn validate_fraction(numerator: u64, denominator: u64) -> Result<(), SwapError> {
    if denominator == 0 && numerator == 0 {
        Ok(())
    } else if numerator >= denominator {
        Err(SwapError::InvalidFee)
    } else {
        Ok(())
    }
}

impl Fees {
    /// Calculate the withdraw fee in pool tokens
    pub fn owner_withdraw_fee(&self, pool_tokens: u128) -> Option<u128> {
        calculate_fee(
            pool_tokens,
            u128::from(self.owner_withdraw_fee_numerator),
            u128::from(self.owner_withdraw_fee_denominator),
        )
    }

    /// Calculate the trading fee in trading tokens 计算交易token 费用
    pub fn trading_fee(&self, trading_tokens: u128) -> Option<u128> {
        calculate_fee(
            trading_tokens,
            u128::from(self.trade_fee_numerator),
            u128::from(self.trade_fee_denominator),
        )
    }

    /// Calculate the owner trading fee in trading tokens
    pub fn owner_trading_fee(&self, trading_tokens: u128) -> Option<u128> {
        calculate_fee(
            trading_tokens,
            u128::from(self.owner_trade_fee_numerator),
            u128::from(self.owner_trade_fee_denominator),
        )
    }

    /// Calculate the inverse trading amount, how much input is needed to give
    /// the provided output
    pub fn pre_trading_fee_amount(&self, post_fee_amount: u128) -> Option<u128> {
        if self.trade_fee_numerator == 0 || self.trade_fee_denominator == 0 {
            pre_fee_amount(
                post_fee_amount,
                self.owner_trade_fee_numerator as u128,
                self.owner_trade_fee_denominator as u128,
            )
        } else if self.owner_trade_fee_numerator == 0 || self.owner_trade_fee_denominator == 0 {
            pre_fee_amount(
                post_fee_amount,
                self.trade_fee_numerator as u128,
                self.trade_fee_denominator as u128,
            )
        } else {
            pre_fee_amount(
                post_fee_amount,
                (self.trade_fee_numerator as u128)
                    .checked_mul(self.owner_trade_fee_denominator as u128)?
                    .checked_add(
                        (self.owner_trade_fee_numerator as u128)
                            .checked_mul(self.trade_fee_denominator as u128)?,
                    )?,
                (self.trade_fee_denominator as u128)
                    .checked_mul(self.owner_trade_fee_denominator as u128)?,
            )
        }
    }

    /// Calculate the host fee based on the owner fee, only used in production
    /// situations where a program is hosted by multiple frontends
    pub fn host_fee(&self, owner_fee: u128) -> Option<u128> {
        calculate_fee(
            owner_fee,
            u128::from(self.host_fee_numerator),
            u128::from(self.host_fee_denominator),
        )
    }

    /// Validate that the fees are reasonable
    pub fn validate(&self) -> Result<(), SwapError> {
        validate_fraction(self.trade_fee_numerator, self.trade_fee_denominator)?;
        validate_fraction(
            self.owner_trade_fee_numerator,
            self.owner_trade_fee_denominator,
        )?;
        validate_fraction(
            self.owner_withdraw_fee_numerator,
            self.owner_withdraw_fee_denominator,
        )?;
        validate_fraction(self.host_fee_numerator, self.host_fee_denominator)?;
        Ok(())
    }
}

/// IsInitialized is required to use `Pack::pack` and `Pack::unpack`
impl IsInitialized for Fees {
    fn is_initialized(&self) -> bool {
        true
    }
}

impl Sealed for Fees {}
impl Pack for Fees {
    const LEN: usize = 64;
    fn pack_into_slice(&self, output: &mut [u8]) {
        let output = array_mut_ref![output, 0, 64];
        let (
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        ) = mut_array_refs![output, 8, 8, 8, 8, 8, 8, 8, 8];
        *trade_fee_numerator = self.trade_fee_numerator.to_le_bytes();
        *trade_fee_denominator = self.trade_fee_denominator.to_le_bytes();
        *owner_trade_fee_numerator = self.owner_trade_fee_numerator.to_le_bytes();
        *owner_trade_fee_denominator = self.owner_trade_fee_denominator.to_le_bytes();
        *owner_withdraw_fee_numerator = self.owner_withdraw_fee_numerator.to_le_bytes();
        *owner_withdraw_fee_denominator = self.owner_withdraw_fee_denominator.to_le_bytes();
        *host_fee_numerator = self.host_fee_numerator.to_le_bytes();
        *host_fee_denominator = self.host_fee_denominator.to_le_bytes();
    }

    fn unpack_from_slice(input: &[u8]) -> Result<Fees, ProgramError> {
        let input = array_ref![input, 0, 64];
        #[allow(clippy::ptr_offset_with_cast)]
        let (
            trade_fee_numerator,
            trade_fee_denominator,
            owner_trade_fee_numerator,
            owner_trade_fee_denominator,
            owner_withdraw_fee_numerator,
            owner_withdraw_fee_denominator,
            host_fee_numerator,
            host_fee_denominator,
        ) = array_refs![input, 8, 8, 8, 8, 8, 8, 8, 8];
        Ok(Self {
            trade_fee_numerator: u64::from_le_bytes(*trade_fee_numerator),
            trade_fee_denominator: u64::from_le_bytes(*trade_fee_denominator),
            owner_trade_fee_numerator: u64::from_le_bytes(*owner_trade_fee_numerator),
            owner_trade_fee_denominator: u64::from_le_bytes(*owner_trade_fee_denominator),
            owner_withdraw_fee_numerator: u64::from_le_bytes(*owner_withdraw_fee_numerator),
            owner_withdraw_fee_denominator: u64::from_le_bytes(*owner_withdraw_fee_denominator),
            host_fee_numerator: u64::from_le_bytes(*host_fee_numerator),
            host_fee_denominator: u64::from_le_bytes(*host_fee_denominator),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_fees() {
        let trade_fee_numerator = 1;
        let trade_fee_denominator = 4;
        let owner_trade_fee_numerator = 2;
        let owner_trade_fee_denominator = 5;
        let owner_withdraw_fee_numerator = 4;
        let owner_withdraw_fee_denominator = 10;
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

        let mut packed = [0u8; Fees::LEN];
        Pack::pack_into_slice(&fees, &mut packed[..]);
        let unpacked = Fees::unpack_from_slice(&packed).unwrap();
        assert_eq!(fees, unpacked);

        let mut packed = vec![];
        packed.extend_from_slice(&trade_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&trade_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&owner_trade_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&owner_trade_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&owner_withdraw_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&owner_withdraw_fee_denominator.to_le_bytes());
        packed.extend_from_slice(&host_fee_numerator.to_le_bytes());
        packed.extend_from_slice(&host_fee_denominator.to_le_bytes());
        let unpacked = Fees::unpack_from_slice(&packed).unwrap();
        assert_eq!(fees, unpacked);
    }
}
