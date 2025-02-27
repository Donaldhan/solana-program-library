//! Simple constant price swap curve, set at init
use {
    crate::{
        curve::calculator::{
            map_zero_to_none, CurveCalculator, DynPack, RoundDirection, SwapWithoutFeesResult,
            TradeDirection, TradingTokenResult,
        },
        error::SwapError,
    },
    arrayref::{array_mut_ref, array_ref},
    solana_program::{
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
    },
    spl_math::{checked_ceil_div::CheckedCeilDiv, precise_number::PreciseNumber, uint::U256},
};

/// Get the amount of pool tokens for the given amount of token A or B.
///
/// The constant product implementation uses the Balancer formulas found at
/// <https://balancer.finance/whitepaper/#single-asset-deposit>, specifically
/// in the case for 2 tokens, each weighted at 1/2.
pub fn trading_tokens_to_pool_tokens(
    token_b_price: u64,
    source_amount: u128,
    swap_token_a_amount: u128,
    swap_token_b_amount: u128,
    pool_supply: u128,
    trade_direction: TradeDirection,
    round_direction: RoundDirection,
) -> Option<u128> {
    let token_b_price = U256::from(token_b_price);
    // 计算存入代币的总价值
    let given_value = match trade_direction {
        // 如果用户存入的是 Token A，那么 given_value = source_amount（直接使用数量）。
        TradeDirection::AtoB => U256::from(source_amount),
        // 如果用户存入的是 Token B，由于 Token B 需要换算成 Token A 价值：
        TradeDirection::BtoA => U256::from(source_amount).checked_mul(token_b_price)?,
    };
    // 计算池子里（Token A + Token B）的总价值，全部 Token A 和 Token B 转换为 Token A 计价后的总价值。
    let total_value = U256::from(swap_token_b_amount)
        .checked_mul(token_b_price)?
        .checked_add(U256::from(swap_token_a_amount))?;
    let pool_supply = U256::from(pool_supply);
    // •	这是一个 线性比例公式，表示用户的存款占整个池子总价值的比例，然后按照这个比例给用户分配 LP 代币。
    // •	checked_div()：执行整除运算（向下取整）。
    // •	checked_ceil_div()：执行向上取整的整除运算，确保计算不会因小数丢失导致用户获得的 LP 代币不足。
    match round_direction {
        RoundDirection::Floor => Some(
            pool_supply
                .checked_mul(given_value)?
                .checked_div(total_value)?
                .as_u128(),
        ),
        RoundDirection::Ceiling => Some(
            pool_supply
                .checked_mul(given_value)?
                .checked_ceil_div(total_value)?
                .0
                .as_u128(),
        ),
    }
}

/// ConstantPriceCurve struct implementing CurveCalculator
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstantPriceCurve {
    /// Amount of token A required to get 1 token B
    pub token_b_price: u64,
}

impl CurveCalculator for ConstantPriceCurve {
    /// Constant price curve always returns 1:1
    /// 这个 swap_without_fees 函数计算了代币交换的过程，具体步骤如下：
    // 1.	根据交易方向（BtoA 或 AtoB），计算交换后的源代币和目标代币数量。
    // 2.	对于 AtoB 交易方向，如果有余数，则向下取整源代币数量，以避免多扣除源代币。
    // 3.	使用 map_zero_to_none 处理零值，确保没有实际交换的情况下返回 None。
    // 4.	返回包含交换结果的 SwapWithoutFeesResult 对象。

    fn swap_without_fees(
        &self,
        source_amount: u128,
        _swap_source_amount: u128,
        _swap_destination_amount: u128,
        trade_direction: TradeDirection,
    ) -> Option<SwapWithoutFeesResult> {
        let token_b_price = self.token_b_price as u128;

        let (source_amount_swapped, destination_amount_swapped) = match trade_direction {
            TradeDirection::BtoA => (source_amount, source_amount.checked_mul(token_b_price)?),
            TradeDirection::AtoB => {
                let destination_amount_swapped = source_amount.checked_div(token_b_price)?;
                let mut source_amount_swapped = source_amount;

                // if there is a remainder from buying token B, floor
                // token_a_amount to avoid taking too many tokens, but
                // don't recalculate the fees
                // 如果在从 A 转换到 B 时，源代币 A 数量除以目标代币 B 的价格时有余数（即无法完全换成目标代币 B），那么我们会将源代币 A 的数量向下取整，去掉余数。
                let remainder = source_amount_swapped.checked_rem(token_b_price)?;
                if remainder > 0 {
                    source_amount_swapped = source_amount.checked_sub(remainder)?;
                }

                (source_amount_swapped, destination_amount_swapped)
            }
        };
        // 这里使用了一个名为 map_zero_to_none 的辅助函数（假设是自定义的），用于将零值转换为 None。即如果交换后的源代币或目标代币数量为 0，则返回 None，表示没有实际的交换发生。
        let source_amount_swapped = map_zero_to_none(source_amount_swapped)?;
        let destination_amount_swapped = map_zero_to_none(destination_amount_swapped)?;
        Some(SwapWithoutFeesResult {
            source_amount_swapped,
            destination_amount_swapped,
        })
    }

    /// Get the amount of trading tokens for the given amount of pool tokens,
    /// provided the total trading tokens and supply of pool tokens.
    /// For the constant price curve, the total value of the pool is weighted
    /// by the price of token B.
    fn pool_tokens_to_trading_tokens(
        &self,
        pool_tokens: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        round_direction: RoundDirection,
    ) -> Option<TradingTokenResult> {
        // self.token_b_price as u128：获取代币 B 的价格，通常它是一个浮动的值（例如单位价格），但在这里它被转换为 u128 类型，用于后续计算。
        // •	normalized_value(swap_token_a_amount, swap_token_b_amount)：这是一个调用了 normalized_value 方法的操作，该方法计算代币 A 和代币 B 在池中的加权值。这个方法返回一个值，它是经过规范化的池内资产的价值。
        // •	.to_imprecise()?：将规范化的值转化为不太精确的值。可能涉及到浮动或舍入，但要注意，这一方法的细节没有给出。
        let token_b_price = self.token_b_price as u128;
        let total_value = self
            .normalized_value(swap_token_a_amount, swap_token_b_amount)?
            .to_imprecise()?;

        let (token_a_amount, token_b_amount) = match round_direction {
            RoundDirection::Floor => {
                let token_a_amount = pool_tokens
                    .checked_mul(total_value)?
                    .checked_div(pool_token_supply)?;
                let token_b_amount = pool_tokens
                    .checked_mul(total_value)?
                    .checked_div(token_b_price)?
                    .checked_div(pool_token_supply)?;
                (token_a_amount, token_b_amount)
            }
            // 向上取整 (Ceiling)
            RoundDirection::Ceiling => {
                let (token_a_amount, _) = pool_tokens
                    .checked_mul(total_value)?
                    .checked_ceil_div(pool_token_supply)?;
                let (pool_value_as_token_b, _) = pool_tokens
                    .checked_mul(total_value)?
                    .checked_ceil_div(token_b_price)?;
                let (token_b_amount, _) =
                    pool_value_as_token_b.checked_ceil_div(pool_token_supply)?;
                (token_a_amount, token_b_amount)
            }
        };
        Some(TradingTokenResult {
            token_a_amount,
            token_b_amount,
        })
    }

    /// Get the amount of pool tokens for the given amount of token A and B
    /// For the constant price curve, the total value of the pool is weighted
    /// by the price of token B.
    fn deposit_single_token_type(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128> {
        trading_tokens_to_pool_tokens(
            self.token_b_price,
            source_amount,
            swap_token_a_amount,
            swap_token_b_amount,
            pool_supply,
            trade_direction,
            RoundDirection::Floor,
        )
    }
    // withdraw_single_token_type_exact_out 计算取款时销毁的 LP 代币
    fn withdraw_single_token_type_exact_out(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
        round_direction: RoundDirection,
    ) -> Option<u128> {
        trading_tokens_to_pool_tokens(
            self.token_b_price,
            source_amount,
            swap_token_a_amount,
            swap_token_b_amount,
            pool_supply,
            trade_direction,
            round_direction,
        )
    }

    fn validate(&self) -> Result<(), SwapError> {
        if self.token_b_price == 0 {
            Err(SwapError::InvalidCurve)
        } else {
            Ok(())
        }
    }

    fn validate_supply(&self, token_a_amount: u64, _token_b_amount: u64) -> Result<(), SwapError> {
        if token_a_amount == 0 {
            return Err(SwapError::EmptySupply);
        }
        Ok(())
    }

    /// The total normalized value of the constant price curve adds the total
    /// value of the token B side to the token A side.
    ///
    /// Note that since most other curves use a multiplicative invariant, ie.
    /// `token_a * token_b`, whereas this one uses an addition,
    /// ie. `token_a + token_b`.
    ///
    /// At the end, we divide by 2 to normalize the value between the two token
    /// types.
    fn normalized_value(
        &self,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) -> Option<PreciseNumber> {
        // 计算代币 B 的总价值
        let swap_token_b_value = swap_token_b_amount.checked_mul(self.token_b_price as u128)?;
        // special logic in case we're close to the limits, avoid overflowing u128
        let value = if swap_token_b_value.saturating_sub(u64::MAX.into())
            > (u128::MAX.saturating_sub(u64::MAX.into()))
        {
            //         这段代码是为了防止在数值接近 u128::MAX 时溢出（因为 u128::MAX 是一个非常大的值，直接操作可能导致溢出），所以做了一些特殊处理：
            // •	swap_token_b_value.saturating_sub(u64::MAX.into())：首先检查 swap_token_b_value 是否接近 u128::MAX 的限制。
            // 如果 swap_token_b_value 减去 u64::MAX 后的值大于剩余的最大可用空间（u128::MAX - u64::MAX），则执行特别的处理。
            // •	checked_div(2) 和 checked_add：为了避免溢出，先将 swap_token_b_value 和 swap_token_a_amount 分别除以 2，然后再相加，确保不会超过 u128::MAX。
            swap_token_b_value
                .checked_div(2)?
                .checked_add(swap_token_a_amount.checked_div(2)?)?
        } else {
            // 否则：如果数值没有接近溢出限制，则直接将代币 A 和代币 B 的值相加，并将总和除以 2（即取平均值），得到池内资产的“规范化值”。
            swap_token_a_amount
                .checked_add(swap_token_b_value)?
                .checked_div(2)?
        };
        PreciseNumber::new(value)
    }
}

/// IsInitialized is required to use `Pack::pack` and `Pack::unpack`
impl IsInitialized for ConstantPriceCurve {
    fn is_initialized(&self) -> bool {
        true
    }
}
impl Sealed for ConstantPriceCurve {}
impl Pack for ConstantPriceCurve {
    const LEN: usize = 8;
    fn pack_into_slice(&self, output: &mut [u8]) {
        (self as &dyn DynPack).pack_into_slice(output);
    }

    fn unpack_from_slice(input: &[u8]) -> Result<ConstantPriceCurve, ProgramError> {
        let token_b_price = array_ref![input, 0, 8];
        Ok(Self {
            token_b_price: u64::from_le_bytes(*token_b_price),
        })
    }
}

impl DynPack for ConstantPriceCurve {
    fn pack_into_slice(&self, output: &mut [u8]) {
        let token_b_price = array_mut_ref![output, 0, 8];
        *token_b_price = self.token_b_price.to_le_bytes();
    }
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::curve::calculator::{
            test::{
                check_curve_value_from_swap, check_deposit_token_conversion,
                check_withdraw_token_conversion, total_and_intermediate,
                CONVERSION_BASIS_POINTS_GUARANTEE,
            },
            INITIAL_SWAP_POOL_AMOUNT,
        },
        proptest::prelude::*,
    };

    #[test]
    fn swap_calculation_no_price() {
        let swap_source_amount: u128 = 0;
        let swap_destination_amount: u128 = 0;
        let source_amount: u128 = 100;
        let token_b_price = 1;
        let curve = ConstantPriceCurve { token_b_price };

        let expected_result = SwapWithoutFeesResult {
            source_amount_swapped: source_amount,
            destination_amount_swapped: source_amount,
        };

        let result = curve
            .swap_without_fees(
                source_amount,
                swap_source_amount,
                swap_destination_amount,
                TradeDirection::AtoB,
            )
            .unwrap();
        assert_eq!(result, expected_result);

        let result = curve
            .swap_without_fees(
                source_amount,
                swap_source_amount,
                swap_destination_amount,
                TradeDirection::BtoA,
            )
            .unwrap();
        assert_eq!(result, expected_result);
    }

    #[test]
    fn pack_flat_curve() {
        let token_b_price = 1_251_258;
        let curve = ConstantPriceCurve { token_b_price };

        let mut packed = [0u8; ConstantPriceCurve::LEN];
        Pack::pack_into_slice(&curve, &mut packed[..]);
        let unpacked = ConstantPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);

        let mut packed = vec![];
        packed.extend_from_slice(&token_b_price.to_le_bytes());
        let unpacked = ConstantPriceCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);
    }

    #[test]
    fn swap_calculation_large_price() {
        let token_b_price = 1123513u128;
        let curve = ConstantPriceCurve {
            token_b_price: token_b_price as u64,
        };
        let token_b_amount = 500u128;
        let token_a_amount = token_b_amount * token_b_price;
        let bad_result = curve.swap_without_fees(
            token_b_price - 1u128,
            token_a_amount,
            token_b_amount,
            TradeDirection::AtoB,
        );
        assert!(bad_result.is_none());
        let bad_result =
            curve.swap_without_fees(1u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
        assert!(bad_result.is_none());
        let result = curve
            .swap_without_fees(
                token_b_price,
                token_a_amount,
                token_b_amount,
                TradeDirection::AtoB,
            )
            .unwrap();
        assert_eq!(result.source_amount_swapped, token_b_price);
        assert_eq!(result.destination_amount_swapped, 1u128);
    }

    #[test]
    fn swap_calculation_max_min() {
        let token_b_price = u64::MAX as u128;
        let curve = ConstantPriceCurve {
            token_b_price: token_b_price as u64,
        };
        let token_b_amount = 1u128;
        let token_a_amount = token_b_price;
        let bad_result = curve.swap_without_fees(
            token_b_price - 1u128,
            token_a_amount,
            token_b_amount,
            TradeDirection::AtoB,
        );
        assert!(bad_result.is_none());
        let bad_result =
            curve.swap_without_fees(1u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
        assert!(bad_result.is_none());
        let bad_result =
            curve.swap_without_fees(0u128, token_a_amount, token_b_amount, TradeDirection::AtoB);
        assert!(bad_result.is_none());
        let result = curve
            .swap_without_fees(
                token_b_price,
                token_a_amount,
                token_b_amount,
                TradeDirection::AtoB,
            )
            .unwrap();
        assert_eq!(result.source_amount_swapped, token_b_price);
        assert_eq!(result.destination_amount_swapped, 1u128);
    }

    proptest! {
        #[test]
        fn deposit_token_conversion_a_to_b(
            // in the pool token conversion calcs, we simulate trading half of
            // source_token_amount, so this needs to be at least 2
            source_token_amount in 2..u64::MAX,
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
            pool_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
            token_b_price in 1..u64::MAX,
        ) {
            let traded_source_amount = source_token_amount / 2;
            // Make sure that the trade yields at least 1 token B
            prop_assume!(traded_source_amount / token_b_price >= 1);
            // Make sure there's enough tokens to get back on the other side
            prop_assume!(traded_source_amount / token_b_price <= swap_destination_amount);

            let curve = ConstantPriceCurve {
                token_b_price,
            };
            check_deposit_token_conversion(
                &curve,
                source_token_amount as u128,
                swap_source_amount as u128,
                swap_destination_amount as u128,
                TradeDirection::AtoB,
                pool_supply,
                CONVERSION_BASIS_POINTS_GUARANTEE,
            );
        }
    }

    proptest! {
        #[test]
        fn deposit_token_conversion_b_to_a(
            // in the pool token conversion calcs, we simulate trading half of
            // source_token_amount, so this needs to be at least 2
            source_token_amount in 2..u32::MAX, // kept small to avoid proptest rejections
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
            pool_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
            token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
        ) {
            let curve = ConstantPriceCurve {
                token_b_price: token_b_price as u64,
            };
            let token_b_price = token_b_price as u128;
            let source_token_amount = source_token_amount as u128;
            let swap_source_amount = swap_source_amount as u128;
            let swap_destination_amount = swap_destination_amount as u128;
            // The constant price curve needs to have enough destination amount
            // on the other side to complete the swap
            prop_assume!(token_b_price * source_token_amount / 2 <= swap_destination_amount);

            check_deposit_token_conversion(
                &curve,
                source_token_amount,
                swap_source_amount,
                swap_destination_amount,
                TradeDirection::BtoA,
                pool_supply,
                CONVERSION_BASIS_POINTS_GUARANTEE,
            );
        }
    }

    proptest! {
        #[test]
        fn withdraw_token_conversion(
            (pool_token_supply, pool_token_amount) in total_and_intermediate(u64::MAX),
            swap_token_a_amount in 1..u64::MAX,
            swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
            token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
        ) {
            let curve = ConstantPriceCurve {
                token_b_price: token_b_price as u64,
            };
            let token_b_price = token_b_price as u128;
            let pool_token_amount = pool_token_amount as u128;
            let pool_token_supply = pool_token_supply as u128;
            let swap_token_a_amount = swap_token_a_amount as u128;
            let swap_token_b_amount = swap_token_b_amount as u128;

            let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

            // Make sure we trade at least one of each token
            prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);

            let withdraw_result = curve
                .pool_tokens_to_trading_tokens(
                    pool_token_amount,
                    pool_token_supply,
                    swap_token_a_amount,
                    swap_token_b_amount,
                    RoundDirection::Floor,
                )
                .unwrap();
            prop_assume!(withdraw_result.token_a_amount <= swap_token_a_amount);
            prop_assume!(withdraw_result.token_b_amount <= swap_token_b_amount);

            check_withdraw_token_conversion(
                &curve,
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
                TradeDirection::AtoB,
                // TODO see why this needs to be so high
                CONVERSION_BASIS_POINTS_GUARANTEE * 20
            );
            check_withdraw_token_conversion(
                &curve,
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
                TradeDirection::BtoA,
                // TODO see why this needs to be so high
                CONVERSION_BASIS_POINTS_GUARANTEE * 20
            );
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_swap_a_to_b(
            source_token_amount in 1..u64::MAX,
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
            token_b_price in 1..u64::MAX,
        ) {
            // Make sure that the trade yields at least 1 token B
            prop_assume!(source_token_amount / token_b_price >= 1);
            // Make sure there's enough tokens to get back on the other side
            prop_assume!(source_token_amount / token_b_price <= swap_destination_amount);
            let curve = ConstantPriceCurve { token_b_price };
            check_curve_value_from_swap(
                &curve,
                source_token_amount as u128,
                swap_source_amount as u128,
                swap_destination_amount as u128,
                TradeDirection::AtoB
            );
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_swap_b_to_a(
            source_token_amount in 1..u32::MAX, // kept small to avoid proptest rejections
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
            token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
        ) {
            // The constant price curve needs to have enough destination amount
            // on the other side to complete the swap
            let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
            let token_b_price = token_b_price as u128;
            let source_token_amount = source_token_amount as u128;
            let swap_destination_amount = swap_destination_amount as u128;
            let swap_source_amount = swap_source_amount as u128;
            // The constant price curve needs to have enough destination amount
            // on the other side to complete the swap
            prop_assume!(token_b_price * source_token_amount <= swap_destination_amount);
            check_curve_value_from_swap(
                &curve,
                source_token_amount,
                swap_source_amount,
                swap_destination_amount,
                TradeDirection::BtoA
            );
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_deposit(
            pool_token_amount in 2..u64::MAX, // minimum 2 to splitting on deposit
            pool_token_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
            swap_token_a_amount in 1..u64::MAX,
            swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
            token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
        ) {
            let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
            let pool_token_amount = pool_token_amount as u128;
            let swap_token_a_amount = swap_token_a_amount as u128;
            let swap_token_b_amount = swap_token_b_amount as u128;
            let token_b_price = token_b_price as u128;

            let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

            // Make sure we trade at least one of each token
            prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);
            let deposit_result = curve
                .pool_tokens_to_trading_tokens(
                    pool_token_amount,
                    pool_token_supply,
                    swap_token_a_amount,
                    swap_token_b_amount,
                    RoundDirection::Ceiling
                )
                .unwrap();
            let new_swap_token_a_amount = swap_token_a_amount + deposit_result.token_a_amount;
            let new_swap_token_b_amount = swap_token_b_amount + deposit_result.token_b_amount;
            let new_pool_token_supply = pool_token_supply + pool_token_amount;

            let new_value = curve.normalized_value(new_swap_token_a_amount, new_swap_token_b_amount).unwrap();

            // the following inequality must hold:
            // new_value / new_pool_token_supply >= value / pool_token_supply
            // which reduces to:
            // new_value * pool_token_supply >= value * new_pool_token_supply

            let pool_token_supply = PreciseNumber::new(pool_token_supply).unwrap();
            let new_pool_token_supply = PreciseNumber::new(new_pool_token_supply).unwrap();
            //let value = U256::from(value);
            //let new_value = U256::from(new_value);

            assert!(new_value.checked_mul(&pool_token_supply).unwrap().greater_than_or_equal(&value.checked_mul(&new_pool_token_supply).unwrap()));
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_withdraw(
            (pool_token_supply, pool_token_amount) in total_and_intermediate(u64::MAX),
            swap_token_a_amount in 1..u64::MAX,
            swap_token_b_amount in 1..u32::MAX, // kept small to avoid proptest rejections
            token_b_price in 1..u32::MAX, // kept small to avoid proptest rejections
        ) {
            let curve = ConstantPriceCurve { token_b_price: token_b_price as u64 };
            let pool_token_amount = pool_token_amount as u128;
            let pool_token_supply = pool_token_supply as u128;
            let swap_token_a_amount = swap_token_a_amount as u128;
            let swap_token_b_amount = swap_token_b_amount as u128;
            let token_b_price = token_b_price as u128;

            let value = curve.normalized_value(swap_token_a_amount, swap_token_b_amount).unwrap();

            // Make sure we trade at least one of each token
            prop_assume!(pool_token_amount * value.to_imprecise().unwrap() >= 2 * token_b_price * pool_token_supply);
            prop_assume!(pool_token_amount <= pool_token_supply);
            let withdraw_result = curve
                .pool_tokens_to_trading_tokens(
                    pool_token_amount,
                    pool_token_supply,
                    swap_token_a_amount,
                    swap_token_b_amount,
                    RoundDirection::Floor,
                )
                .unwrap();
            prop_assume!(withdraw_result.token_a_amount <= swap_token_a_amount);
            prop_assume!(withdraw_result.token_b_amount <= swap_token_b_amount);
            let new_swap_token_a_amount = swap_token_a_amount - withdraw_result.token_a_amount;
            let new_swap_token_b_amount = swap_token_b_amount - withdraw_result.token_b_amount;
            let new_pool_token_supply = pool_token_supply - pool_token_amount;

            let new_value = curve.normalized_value(new_swap_token_a_amount, new_swap_token_b_amount).unwrap();

            // the following inequality must hold:
            // new_value / new_pool_token_supply >= value / pool_token_supply
            // which reduces to:
            // new_value * pool_token_supply >= value * new_pool_token_supply

            let pool_token_supply = PreciseNumber::new(pool_token_supply).unwrap();
            let new_pool_token_supply = PreciseNumber::new(new_pool_token_supply).unwrap();
            assert!(new_value.checked_mul(&pool_token_supply).unwrap().greater_than_or_equal(&value.checked_mul(&new_pool_token_supply).unwrap()));
        }
    }
}
