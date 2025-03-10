//! The Uniswap invariant calculator.

use {
    crate::{
        curve::calculator::{
            map_zero_to_none, CurveCalculator, DynPack, RoundDirection, SwapWithoutFeesResult,
            TradeDirection, TradingTokenResult,
        },
        error::SwapError,
    },
    solana_program::{
        program_error::ProgramError,
        program_pack::{IsInitialized, Pack, Sealed},
    },
    spl_math::{checked_ceil_div::CheckedCeilDiv, precise_number::PreciseNumber},
};

/// ConstantProductCurve struct implementing CurveCalculator
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstantProductCurve;

/// The constant product swap calculation, factored out of its class for reuse.
///
/// This is guaranteed to work for all values such that:
///  - 1 <= swap_source_amount * swap_destination_amount <= u128::MAX
///  - 1 <= source_amount <= u64::MAX
/// 	•	source_amount: u128: 这是要交换的源代币数量。
// •	swap_source_amount: u128: 这是交易池中当前的源代币数量。
// •	swap_destination_amount: u128: 这是交易池中当前的目标代币数量。
// •	返回值：Option<SwapWithoutFeesResult>，表示一个交换结果对象。Option 类型是为了处理可能失败的情况。


// 这个 swap 函数实现了一个基于恒定乘积模型的代币交换过程，具体步骤如下：
// 	1.	计算当前市场的恒定乘积（源代币数量 × 目标代币数量）。
// 	2.	计算新的源代币数量，即现有源代币数量加上用户希望交换的源代币数量。
// 	3.	根据恒定乘积和新的源代币数量计算新的目标代币数量。
// 	4.	计算实际交换的源代币数量（新源代币数量 - 原源代币数量）。
// 	5.	计算实际交换的目标代币数量（原目标代币数量 - 新目标代币数量）。
// 	6.	返回交换结果对象，包括源代币和目标代币的交换数量。

// 如果在任何计算过程中出现溢出或错误（例如除法失败），函数将返回 None，否则返回一个包含实际交换数量的 SwapWithoutFeesResult 对象。
pub fn swap(
    source_amount: u128,
    swap_source_amount: u128,
    swap_destination_amount: u128,
) -> Option<SwapWithoutFeesResult> {
    let invariant = swap_source_amount.checked_mul(swap_destination_amount)?;

    let new_swap_source_amount = swap_source_amount.checked_add(source_amount)?;
    let (new_swap_destination_amount, new_swap_source_amount) =
        invariant.checked_ceil_div(new_swap_source_amount)?;

    let source_amount_swapped = new_swap_source_amount.checked_sub(swap_source_amount)?;
    let destination_amount_swapped =
        map_zero_to_none(swap_destination_amount.checked_sub(new_swap_destination_amount)?)?;

    Some(SwapWithoutFeesResult {
        source_amount_swapped,
        destination_amount_swapped,
    })
}

/// Get the amount of trading tokens for the given amount of pool tokens,
/// provided the total trading tokens and supply of pool tokens.
///
/// The constant product implementation is a simple ratio calculation for how
/// many trading tokens correspond to a certain number of pool tokens
/// 用于根据池代币数量、池代币供应量、交换池中代币的数量，以及四舍五入的方向，
/// 将池代币转换为交易代币的数量（即，计算从流动性池中提取多少代币）。函数返回一个 Option<TradingTokenResult>，
/// 如果计算成功，则返回包含代币 A 和代币 B 数量的 TradingTokenResult，否则返回 None。

pub fn pool_tokens_to_trading_tokens(
    pool_tokens: u128,
    pool_token_supply: u128,
    swap_token_a_amount: u128,
    swap_token_b_amount: u128,
    round_direction: RoundDirection,
) -> Option<TradingTokenResult> {
    // •	计算过程：
	// •	对于代币 A，首先计算池代币与池中代币 A 数量的比例：pool_tokens * swap_token_a_amount / pool_token_supply。这表示根据用户请求的池代币数量，计算出相应的代币 A 数量。
	// •	对于代币 B，执行相同的计算过程：pool_tokens * swap_token_b_amount / pool_token_supply。
	// •	checked_mul 和 checked_div：这两个方法用于安全地执行乘法和除法运算。如果发生溢出或除以零等错误，checked_mul 和 checked_div 会返回 None，导致整个计算提前终止，返回 None。

    let mut token_a_amount = pool_tokens
        .checked_mul(swap_token_a_amount)?
        .checked_div(pool_token_supply)?;
    let mut token_b_amount = pool_tokens
        .checked_mul(swap_token_b_amount)?
        .checked_div(pool_token_supply)?;
    let (token_a_amount, token_b_amount) = match round_direction {
        RoundDirection::Floor => (token_a_amount, token_b_amount),
        RoundDirection::Ceiling => {
            let token_a_remainder = pool_tokens
                .checked_mul(swap_token_a_amount)?
                .checked_rem(pool_token_supply)?;
            // Also check for 0 token A and B amount to avoid taking too much
            // for tiny amounts of pool tokens.  For example, if someone asks
            // for 1 pool token, which is worth 0.01 token A, we avoid the
            // ceiling of taking 1 token A and instead return 0, for it to be
            // rejected later in processing.
            if token_a_remainder > 0 && token_a_amount > 0 {
                token_a_amount += 1;
            }
            let token_b_remainder = pool_tokens
                .checked_mul(swap_token_b_amount)?
                .checked_rem(pool_token_supply)?;
            if token_b_remainder > 0 && token_b_amount > 0 {
                token_b_amount += 1;
            }
            (token_a_amount, token_b_amount)
        }
    };
    Some(TradingTokenResult {
        token_a_amount,
        token_b_amount,
    })
}

/// Get the amount of pool tokens for the deposited amount of token A or B.
///
/// The constant product implementation uses the Balancer formulas found at
/// <https://balancer.finance/whitepaper/#single-asset-deposit>, specifically
/// in the case for 2 tokens, each weighted at 1/2.
pub fn deposit_single_token_type(
    source_amount: u128,
    swap_token_a_amount: u128,
    swap_token_b_amount: u128,
    pool_supply: u128,
    trade_direction: TradeDirection,
    round_direction: RoundDirection,
) -> Option<u128> {
    let swap_source_amount = match trade_direction {
        TradeDirection::AtoB => swap_token_a_amount,
        TradeDirection::BtoA => swap_token_b_amount,
    };
    let swap_source_amount = PreciseNumber::new(swap_source_amount)?;
    let source_amount = PreciseNumber::new(source_amount)?;
    // 计算用户存入的代币数量和当前池中源代币的数量的比例，表示用户存入的代币占池中源代币的比例。
    let ratio = source_amount.checked_div(&swap_source_amount)?;
    let one = PreciseNumber::new(1)?;
    // •	base = 1 + ratio：基于用户存入的代币数量和池中源代币的比例，计算一个基数。
	// •	root = sqrt(base) - 1：计算基数的平方根，并减去 1。这个操作反映了用户存入代币后，在池中所能获得的池子代币数量的比例。
    let base = one.checked_add(&ratio)?;
    let root = base.sqrt()?.checked_sub(&one)?;
    // 计算池子代币数量 (pool_tokens)：
    let pool_supply = PreciseNumber::new(pool_supply)?;
    let pool_tokens = pool_supply.checked_mul(&root)?;
    match round_direction {
        RoundDirection::Floor => pool_tokens.floor()?.to_imprecise(),
        RoundDirection::Ceiling => pool_tokens.ceiling()?.to_imprecise(),
    }
}

/// Get the amount of pool tokens for the withdrawn amount of token A or B.
///
/// The constant product implementation uses the Balancer formulas found at
/// <https://balancer.finance/whitepaper/#single-asset-withdrawal>, specifically
/// in the case for 2 tokens, each weighted at 1/2.
/// 作用
// 	•	该函数用于计算用户精确提取某种代币时，需要销毁的 LP 代币数量。
// 	•	由于 AMM 公式的影响，LP 代币的计算是非线性的，因此需要使用平方根调整比例。

// 计算步骤
// 	1.	确定提取的代币类型（Token A 或 Token B）。
// 	2.	计算提取数量占池子的比例 ratio = 提取数量 / 池子总量。
// 	3.	计算 LP 代币销毁数量：
// 	•	计算 1 - ratio
// 	•	取平方根进行非线性调整
// 	•	乘以 pool_supply 计算最终 LP 代币销毁量
// 	4.	根据 round_direction 进行取整：
// 	•	Floor：向下取整
// 	•	Ceiling：向上取整
// 为什么用平方根？
// 	•	AMM 采用 x * y = k 公式，因此 LP 代币的计算是非线性的。
// 	•	取款影响池子余额，导致 LP 代币的比例变化不是线性关系，需要平方根调整。

// 优化点
// 	•	由于 sqrt 计算相对耗时，可通过 二分法近似计算 提高效率。
// 	•	checked_* 方式确保了数值不会溢出，提高安全性。
pub fn withdraw_single_token_type_exact_out(
    source_amount: u128,
    swap_token_a_amount: u128,
    swap_token_b_amount: u128,
    pool_supply: u128,
    trade_direction: TradeDirection,
    round_direction: RoundDirection,
) -> Option<u128> {
    let swap_source_amount = match trade_direction {
        TradeDirection::AtoB => swap_token_a_amount,
        TradeDirection::BtoA => swap_token_b_amount,
    };
    let swap_source_amount = PreciseNumber::new(swap_source_amount)?;
    let source_amount = PreciseNumber::new(source_amount)?;
    // ratio = source_amount / swap_source_amount，表示用户想提取的数量占池子里该代币总量的比例。
    let ratio = source_amount.checked_div(&swap_source_amount)?;
    // 这部分核心逻辑：
	// 1.	计算 base = 1 - ratio，表示池子里剩余代币的比例。
	// 2.	计算 root = 1 - sqrt(base)：
	// •	由于 AMM 采用乘积恒定公式（x * y = k），LP 代币的计算方式是非线性的，需要使用平方根函数来调整比例。
	// 3.	计算 pool_tokens = pool_supply * root：
	// •	这里 root 代表池子 LP 代币的变化比例，因此 pool_supply * root 计算了需要销毁的 LP 代币数量。

    let one = PreciseNumber::new(1)?;
    let base = one
        .checked_sub(&ratio)
        .unwrap_or_else(|| PreciseNumber::new(0).unwrap());
    let root = one.checked_sub(&base.sqrt()?)?;
    let pool_supply = PreciseNumber::new(pool_supply)?;
    let pool_tokens = pool_supply.checked_mul(&root)?;
    match round_direction {
        RoundDirection::Floor => pool_tokens.floor()?.to_imprecise(),
        RoundDirection::Ceiling => pool_tokens.ceiling()?.to_imprecise(),
    }
}

/// Calculates the total normalized value of the curve given the liquidity
/// parameters.
///
/// The constant product implementation for this function gives the square root
/// of the Uniswap invariant.
pub fn normalized_value(
    swap_token_a_amount: u128,
    swap_token_b_amount: u128,
) -> Option<PreciseNumber> {
    let swap_token_a_amount = PreciseNumber::new(swap_token_a_amount)?;
    let swap_token_b_amount = PreciseNumber::new(swap_token_b_amount)?;
    swap_token_a_amount
        .checked_mul(&swap_token_b_amount)?
        .sqrt()
}

impl CurveCalculator for ConstantProductCurve {
    /// Constant product swap ensures x * y = constant
    fn swap_without_fees(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        _trade_direction: TradeDirection,
    ) -> Option<SwapWithoutFeesResult> {
        swap(source_amount, swap_source_amount, swap_destination_amount)
    }

    /// The constant product implementation is a simple ratio calculation for
    /// how many trading tokens correspond to a certain number of pool
    /// tokens
    fn pool_tokens_to_trading_tokens(
        &self,
        pool_tokens: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        round_direction: RoundDirection,
    ) -> Option<TradingTokenResult> {
        pool_tokens_to_trading_tokens(
            pool_tokens,
            pool_token_supply,
            swap_token_a_amount,
            swap_token_b_amount,
            round_direction,
        )
    }

    /// Get the amount of pool tokens for the deposited amount of token A or B.
    fn deposit_single_token_type(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128> {
        deposit_single_token_type(
            source_amount,
            swap_token_a_amount,
            swap_token_b_amount,
            pool_supply,
            trade_direction,
            RoundDirection::Floor,
        )
    }

    fn withdraw_single_token_type_exact_out(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
        round_direction: RoundDirection,
    ) -> Option<u128> {
        withdraw_single_token_type_exact_out(
            source_amount,
            swap_token_a_amount,
            swap_token_b_amount,
            pool_supply,
            trade_direction,
            round_direction,
        )
    }

    fn normalized_value(
        &self,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) -> Option<PreciseNumber> {
        normalized_value(swap_token_a_amount, swap_token_b_amount)
    }

    fn validate(&self) -> Result<(), SwapError> {
        Ok(())
    }
}

/// IsInitialized is required to use `Pack::pack` and `Pack::unpack`
impl IsInitialized for ConstantProductCurve {
    fn is_initialized(&self) -> bool {
        true
    }
}
impl Sealed for ConstantProductCurve {}
impl Pack for ConstantProductCurve {
    const LEN: usize = 0;
    fn pack_into_slice(&self, output: &mut [u8]) {
        (self as &dyn DynPack).pack_into_slice(output);
    }

    fn unpack_from_slice(_input: &[u8]) -> Result<ConstantProductCurve, ProgramError> {
        Ok(Self {})
    }
}

impl DynPack for ConstantProductCurve {
    fn pack_into_slice(&self, _output: &mut [u8]) {}
}

#[cfg(test)]
mod tests {
    use {
        super::*,
        crate::curve::calculator::{
            test::{
                check_curve_value_from_swap, check_deposit_token_conversion,
                check_pool_value_from_deposit, check_pool_value_from_withdraw,
                check_withdraw_token_conversion, total_and_intermediate,
                CONVERSION_BASIS_POINTS_GUARANTEE,
            },
            RoundDirection, INITIAL_SWAP_POOL_AMOUNT,
        },
        proptest::prelude::*,
    };

    #[test]
    fn initial_pool_amount() {
        let calculator = ConstantProductCurve {};
        assert_eq!(calculator.new_pool_supply(), INITIAL_SWAP_POOL_AMOUNT);
    }

    fn check_pool_token_rate(
        token_a: u128,
        token_b: u128,
        deposit: u128,
        supply: u128,
        expected_a: u128,
        expected_b: u128,
    ) {
        let calculator = ConstantProductCurve {};
        let results = calculator
            .pool_tokens_to_trading_tokens(
                deposit,
                supply,
                token_a,
                token_b,
                RoundDirection::Ceiling,
            )
            .unwrap();
        assert_eq!(results.token_a_amount, expected_a);
        assert_eq!(results.token_b_amount, expected_b);
    }

    #[test]
    fn trading_token_conversion() {
        check_pool_token_rate(2, 49, 5, 10, 1, 25);
        check_pool_token_rate(100, 202, 5, 101, 5, 10);
        check_pool_token_rate(5, 501, 2, 10, 1, 101);
    }

    #[test]
    fn fail_trading_token_conversion() {
        let calculator = ConstantProductCurve {};
        let results =
            calculator.pool_tokens_to_trading_tokens(5, 10, u128::MAX, 0, RoundDirection::Floor);
        assert!(results.is_none());
        let results =
            calculator.pool_tokens_to_trading_tokens(5, 10, 0, u128::MAX, RoundDirection::Floor);
        assert!(results.is_none());
    }

    #[test]
    fn pack_constant_product_curve() {
        let curve = ConstantProductCurve {};

        let mut packed = [0u8; ConstantProductCurve::LEN];
        Pack::pack_into_slice(&curve, &mut packed[..]);
        let unpacked = ConstantProductCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);

        let packed = vec![];
        let unpacked = ConstantProductCurve::unpack(&packed).unwrap();
        assert_eq!(curve, unpacked);
    }

    fn test_truncation(
        curve: &ConstantProductCurve,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        expected_source_amount_swapped: u128,
        expected_destination_amount_swapped: u128,
    ) {
        let invariant = swap_source_amount * swap_destination_amount;
        let result = curve
            .swap_without_fees(
                source_amount,
                swap_source_amount,
                swap_destination_amount,
                TradeDirection::AtoB,
            )
            .unwrap();
        assert_eq!(result.source_amount_swapped, expected_source_amount_swapped);
        assert_eq!(
            result.destination_amount_swapped,
            expected_destination_amount_swapped
        );
        let new_invariant = (swap_source_amount + result.source_amount_swapped)
            * (swap_destination_amount - result.destination_amount_swapped);
        assert!(new_invariant >= invariant);
    }

    #[test]
    fn constant_product_swap_rounding() {
        let curve = ConstantProductCurve;

        // much too small
        assert!(curve
            .swap_without_fees(10, 70_000_000_000, 4_000_000, TradeDirection::AtoB)
            .is_none()); // spot: 10 * 4m / 70b = 0

        let tests: &[(u128, u128, u128, u128, u128)] = &[
            // spot: 10 * 70b / ~4m = 174,999.99
            (10, 4_000_000, 70_000_000_000, 10, 174_999),
            // spot: 20 * 1 / 3.000 = 6.6667 (source can be 18 to get 6 dest.)
            (20, 30_000 - 20, 10_000, 18, 6),
            // spot: 19 * 1 / 2.999 = 6.3334 (source can be 18 to get 6 dest.)
            (19, 30_000 - 20, 10_000, 18, 6),
            // spot: 18 * 1 / 2.999 = 6.0001
            (18, 30_000 - 20, 10_000, 18, 6),
            // spot: 10 * 3 / 2.0010 = 14.99
            (10, 20_000, 30_000, 10, 14),
            // spot: 10 * 3 / 2.0001 = 14.999
            (10, 20_000 - 9, 30_000, 10, 14),
            // spot: 10 * 3 / 2.0000 = 15
            (10, 20_000 - 10, 30_000, 10, 15),
            // spot: 100 * 3 / 6.001 = 49.99 (source can be 99 to get 49 dest.)
            (100, 60_000, 30_000, 99, 49),
            // spot: 99 * 3 / 6.001 = 49.49
            (99, 60_000, 30_000, 99, 49),
            // spot: 98 * 3 / 6.001 = 48.99 (source can be 97 to get 48 dest.)
            (98, 60_000, 30_000, 97, 48),
        ];
        for (
            source_amount,
            swap_source_amount,
            swap_destination_amount,
            expected_source_amount,
            expected_destination_amount,
        ) in tests.iter()
        {
            test_truncation(
                &curve,
                *source_amount,
                *swap_source_amount,
                *swap_destination_amount,
                *expected_source_amount,
                *expected_destination_amount,
            );
        }
    }

    proptest! {
        #[test]
        fn deposit_token_conversion(
            // in the pool token conversion calcs, we simulate trading half of
            // source_token_amount, so this needs to be at least 2
            source_token_amount in 2..u64::MAX,
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
            pool_supply in INITIAL_SWAP_POOL_AMOUNT..u64::MAX as u128,
        ) {
            let curve = ConstantProductCurve {};
            check_deposit_token_conversion(
                &curve,
                source_token_amount as u128,
                swap_source_amount as u128,
                swap_destination_amount as u128,
                TradeDirection::AtoB,
                pool_supply,
                CONVERSION_BASIS_POINTS_GUARANTEE,
            );

            check_deposit_token_conversion(
                &curve,
                source_token_amount as u128,
                swap_source_amount as u128,
                swap_destination_amount as u128,
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
            swap_token_b_amount in 1..u64::MAX,
        ) {
            let curve = ConstantProductCurve {};
            check_withdraw_token_conversion(
                &curve,
                pool_token_amount as u128,
                pool_token_supply as u128,
                swap_token_a_amount as u128,
                swap_token_b_amount as u128,
                TradeDirection::AtoB,
                CONVERSION_BASIS_POINTS_GUARANTEE
            );
            check_withdraw_token_conversion(
                &curve,
                pool_token_amount as u128,
                pool_token_supply as u128,
                swap_token_a_amount as u128,
                swap_token_b_amount as u128,
                TradeDirection::BtoA,
                CONVERSION_BASIS_POINTS_GUARANTEE
            );
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_swap(
            source_token_amount in 1..u64::MAX,
            swap_source_amount in 1..u64::MAX,
            swap_destination_amount in 1..u64::MAX,
        ) {
            let curve = ConstantProductCurve {};
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
        fn curve_value_does_not_decrease_from_deposit(
            pool_token_amount in 1..u64::MAX,
            pool_token_supply in 1..u64::MAX,
            swap_token_a_amount in 1..u64::MAX,
            swap_token_b_amount in 1..u64::MAX,
        ) {
            let pool_token_amount = pool_token_amount as u128;
            let pool_token_supply = pool_token_supply as u128;
            let swap_token_a_amount = swap_token_a_amount as u128;
            let swap_token_b_amount = swap_token_b_amount as u128;
            // Make sure we will get at least one trading token out for each
            // side, otherwise the calculation fails
            prop_assume!(pool_token_amount * swap_token_a_amount / pool_token_supply >= 1);
            prop_assume!(pool_token_amount * swap_token_b_amount / pool_token_supply >= 1);
            let curve = ConstantProductCurve {};
            check_pool_value_from_deposit(
                &curve,
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
            );
        }
    }

    proptest! {
        #[test]
        fn curve_value_does_not_decrease_from_withdraw(
            (pool_token_supply, pool_token_amount) in total_and_intermediate(u64::MAX),
            swap_token_a_amount in 1..u64::MAX,
            swap_token_b_amount in 1..u64::MAX,
        ) {
            let pool_token_amount = pool_token_amount as u128;
            let pool_token_supply = pool_token_supply as u128;
            let swap_token_a_amount = swap_token_a_amount as u128;
            let swap_token_b_amount = swap_token_b_amount as u128;
            // Make sure we will get at least one trading token out for each
            // side, otherwise the calculation fails
            prop_assume!(pool_token_amount * swap_token_a_amount / pool_token_supply >= 1);
            prop_assume!(pool_token_amount * swap_token_b_amount / pool_token_supply >= 1);
            let curve = ConstantProductCurve {};
            check_pool_value_from_withdraw(
                &curve,
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
            );
        }
    }
}
