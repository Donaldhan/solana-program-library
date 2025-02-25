//! Swap calculations

#[cfg(feature = "fuzz")]
use arbitrary::Arbitrary;
use {crate::error::SwapError, spl_math::precise_number::PreciseNumber, std::fmt::Debug};

/// Initial amount of pool tokens for swap contract, hard-coded to something
/// "sensible" given a maximum of u128.
/// Note that on Ethereum, Uniswap uses the geometric mean of all provided
/// input amounts, and Balancer uses 100 * 10 ^ 18.
pub const INITIAL_SWAP_POOL_AMOUNT: u128 = 1_000_000_000;

/// Hardcode the number of token types in a pool, used to calculate the
/// equivalent pool tokens for the owner trading fee.
pub const TOKENS_IN_POOL: u128 = 2;

/// Helper function for mapping to SwapError::CalculationFailure
pub fn map_zero_to_none(x: u128) -> Option<u128> {
    if x == 0 {
        None
    } else {
        Some(x)
    }
}

/// The direction of a trade, since curves can be specialized to treat each
/// token differently (by adding offsets or weights)
#[cfg_attr(feature = "fuzz", derive(Arbitrary))]
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TradeDirection {
    /// Input token A, output token B
    AtoB,
    /// Input token B, output token A
    BtoA,
}

/// The direction to round.  Used for pool token to trading token conversions to
/// avoid losing value on any deposit or withdrawal.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RoundDirection {
    /// Floor the value, ie. 1.9 => 1, 1.1 => 1, 1.5 => 1
    Floor,
    /// Ceiling the value, ie. 1.9 => 2, 1.1 => 2, 1.5 => 2
    Ceiling,
}

impl TradeDirection {
    /// Given a trade direction, gives the opposite direction of the trade, so
    /// A to B becomes B to A, and vice versa
    pub fn opposite(&self) -> TradeDirection {
        match self {
            TradeDirection::AtoB => TradeDirection::BtoA,
            TradeDirection::BtoA => TradeDirection::AtoB,
        }
    }
}

/// Encodes all results of swapping from a source token to a destination token
#[derive(Debug, PartialEq)]
pub struct SwapWithoutFeesResult {
    /// Amount of source token swapped
    pub source_amount_swapped: u128,
    /// Amount of destination token swapped
    pub destination_amount_swapped: u128,
}

/// Encodes results of depositing both sides at once
#[derive(Debug, PartialEq)]
pub struct TradingTokenResult {
    /// Amount of token A
    pub token_a_amount: u128,
    /// Amount of token B
    pub token_b_amount: u128,
}

/// Trait for packing of trait objects, required because structs that implement
/// `Pack` cannot be used as trait objects (as `dyn Pack`).
pub trait DynPack {
    /// Only required function is to pack given a trait object
    fn pack_into_slice(&self, dst: &mut [u8]);
}

/// Trait representing operations required on a swap curve
/// CurveCalculator 主要定义了不同 Swap 曲线所需的计算逻辑，包括：
	// 1.	无费用 Swap 计算（swap_without_fees）。
	// 2.	流动性池初始化供应量（new_pool_supply）。
	// 3.	LP 代币兑换交易代币（pool_tokens_to_trading_tokens）。
	// 4.	单边存款和取款计算（deposit_single_token_type、withdraw_single_token_type_exact_out）。
	// 5.	曲线参数验证（validate、validate_supply）。
	// 6.	是否允许后续存款（allows_deposits）。
	// 7.	计算曲线的总价值（normalized_value）。
pub trait CurveCalculator: Debug + DynPack {
    /// Calculate how much destination token will be provided given an amount
    /// of source token.
    //     功能：
    // 计算在不考虑交易费用的情况下，输入一定数量的 source token 可以获得多少 destination token。
    // 参数解析：
    // 	•	source_amount：用户想要交换的代币数量（输入代币）。
    // 	•	swap_source_amount：池中 source token 的当前储备量。
    // 	•	swap_destination_amount：池中 destination token 的当前储备量。
    // 	•	trade_direction：交易方向（是 TokenA -> TokenB 还是 TokenB -> TokenA）。
    // 返回值：
    // 返回 Option<SwapWithoutFeesResult>，如果计算成功，包含目标代币的数量；如果失败，返回 None。
    fn swap_without_fees(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
    ) -> Option<SwapWithoutFeesResult>;

    /// Get the supply for a new pool
    /// The default implementation is a Balancer-style fixed initial supply
    /// 获取新池子的初始流动性供应量，默认值是 INITIAL_SWAP_POOL_AMOUNT（通常是 Balancer 风格的固定初始供应量）。
    fn new_pool_supply(&self) -> u128 {
        INITIAL_SWAP_POOL_AMOUNT
    }

    /// Get the amount of trading tokens for the given amount of pool tokens,
    /// provided the total trading tokens and supply of pool tokens.
    /// 功能：
    // 根据流动性代币（LP 代币）计算可以提取的交易代币数量。

    // 参数解析：
    // 	•	pool_tokens：要兑换的 LP 代币数量。
    // 	•	pool_token_supply：当前池子的 LP 代币供应量。
    // 	•	swap_token_a_amount / swap_token_b_amount：池子中的 Token A 和 Token B 的当前储备量。
    // 	•	round_direction：四舍五入方向（向上或向下）。

    // 返回值：
    // 返回 TradingTokenResult，表示可以兑换出的 TokenA 或 TokenB 数量。
    fn pool_tokens_to_trading_tokens(
        &self,
        pool_tokens: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        round_direction: RoundDirection,
    ) -> Option<TradingTokenResult>;

    /// Get the amount of pool tokens for the deposited amount of token A or B.
    ///
    /// This is used for single-sided deposits.  It essentially performs a swap
    /// followed by a deposit.  Because a swap is implicitly performed, this
    /// will change the spot price of the pool.
    ///
    /// See more background for the calculation at:
    ///
    /// <https://balancer.finance/whitepaper/#single-asset-deposit-withdrawal>
    /// 功能：
    // 计算用户单边存入一定数量的 TokenA 或 TokenB 后，可以获得多少流动性代币（LP 代币）。

    // 背景知识：
    // 在 Balancer 交易池中，用户可以只存入一种代币，而不是两种代币按比例存入。但这种操作会影响池子的价格曲线。

    // 参数解析：
    // 	•	source_amount：用户存入的 TokenA 或 TokenB 数量。
    // 	•	swap_token_a_amount / swap_token_b_amount：池子中的 Token A 和 Token B 的当前储备量。
    // 	•	pool_supply：当前池子的流动性代币（LP 代币）供应量。
    // 	•	trade_direction：存入的是 TokenA -> Pool 还是 TokenB -> Pool。

    // 返回值：
    // 返回 Option<u128>，表示可以获得的流动性代币数量。
    fn deposit_single_token_type(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
    ) -> Option<u128>;

    /// Get the amount of pool tokens for the withdrawn amount of token A or B.
    ///
    /// This is used for single-sided withdrawals and owner trade fee
    /// calculation. It essentially performs a withdrawal followed by a swap.
    /// Because a swap is implicitly performed, this will change the spot price
    /// of the pool.
    ///
    /// See more background for the calculation at:
    ///
    /// <https://balancer.finance/whitepaper/#single-asset-deposit-withdrawal>
    /// 功能：
    // 计算用户从池子中单边提取一定数量的 TokenA 或 TokenB 需要消耗多少 LP 代币。

    // 参数解析：
    // 	•	source_amount：用户希望提取的 TokenA 或 TokenB 数量。
    // 	•	swap_token_a_amount / swap_token_b_amount：池子的当前储备量。
    // 	•	pool_supply：池子的 LP 代币总供应量。
    // 	•	trade_direction：是 TokenA <- Pool 还是 TokenB <- Pool。
    // 	•	round_direction：四舍五入方向。

    // 返回值：
    // 返回 Option<u128>，表示需要消耗多少 LP 代币。
    fn withdraw_single_token_type_exact_out(
        &self,
        source_amount: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        pool_supply: u128,
        trade_direction: TradeDirection,
        round_direction: RoundDirection,
    ) -> Option<u128>;

    /// Validate that the given curve has no invalid parameters
    /// 检查该曲线是否有非法参数。例如，某些曲线可能不允许某些负值或者非均衡状态。
    fn validate(&self) -> Result<(), SwapError>;

    /// Validate the given supply on initialization. This is useful for curves
    /// that allow zero supply on one or both sides, since the standard constant
    /// product curve must have a non-zero supply on both sides.
    /// 检查池子在初始化时是否有足够的 TokenA 和 TokenB 储备。例如，对于 Uniswap 恒定乘积曲线，初始池子必须有非零的 TokenA 和 TokenB。
    fn validate_supply(&self, token_a_amount: u64, token_b_amount: u64) -> Result<(), SwapError> {
        if token_a_amount == 0 {
            return Err(SwapError::EmptySupply);
        }
        if token_b_amount == 0 {
            return Err(SwapError::EmptySupply);
        }
        Ok(())
    }

    /// Some curves function best and prevent attacks if we prevent deposits
    /// after initialization.  For example, the offset curve in `offset.rs`,
    /// which fakes supply on one side of the swap, allows the swap creator
    /// to steal value from all other depositors.
    /// 某些曲线（如 Offset 曲线）可能不允许池子在初始化后再进行存款，因为可能会造成攻击或套利风险。默认情况下，存款是允许的。
    fn allows_deposits(&self) -> bool {
        true
    }

    /// Calculates the total normalized value of the curve given the liquidity
    /// parameters.
    ///
    /// This value must have the dimension of `tokens ^ 1` For example, the
    /// standard Uniswap invariant has dimension `tokens ^ 2` since we are
    /// multiplying two token values together.  In order to normalize it, we
    /// also need to take the square root.
    ///
    /// This is useful for testing the curves, to make sure that value is not
    /// lost on any trade.  It can also be used to find out the relative value
    /// of pool tokens or liquidity tokens.
    /// 功能：
    // 计算池子的总价值（用于测试和比较不同曲线的公平性）。

    // 背景知识：
    // 	•	Uniswap 使用 x * y = k，k 具有 tokens^2 的维度，所以需要开平方才能归一化。
    // 	•	归一化价值可以用于衡量流动性池的总价值，避免因曲线不合理导致的价值损失。
    fn normalized_value(
        &self,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) -> Option<PreciseNumber>;
}

/// Test helpers for curves
#[cfg(test)]
pub mod test {
    use {super::*, proptest::prelude::*, spl_math::uint::U256};

    /// The epsilon for most curves when performing the conversion test,
    /// comparing a one-sided deposit to a swap + deposit.
    pub const CONVERSION_BASIS_POINTS_GUARANTEE: u128 = 50;

    /// Test function to check that depositing token A is the same as swapping
    /// half for token B and depositing both.
    /// Since calculations use unsigned integers, there will be truncation at
    /// some point, meaning we can't have perfect equality.
    /// We guarantee that the relative error between depositing one side and
    /// performing a swap plus deposit will be at most some epsilon provided by
    /// the curve. Most curves guarantee accuracy within 0.5%.
    pub fn check_deposit_token_conversion(
        curve: &dyn CurveCalculator,
        source_token_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
        pool_supply: u128,
        epsilon_in_basis_points: u128,
    ) {
        let amount_to_swap = source_token_amount / 2;
        let results = curve
            .swap_without_fees(
                amount_to_swap,
                swap_source_amount,
                swap_destination_amount,
                trade_direction,
            )
            .unwrap();
        let opposite_direction = trade_direction.opposite();
        let (swap_token_a_amount, swap_token_b_amount) = match trade_direction {
            TradeDirection::AtoB => (swap_source_amount, swap_destination_amount),
            TradeDirection::BtoA => (swap_destination_amount, swap_source_amount),
        };

        // base amount
        let pool_tokens_from_one_side = curve
            .deposit_single_token_type(
                source_token_amount,
                swap_token_a_amount,
                swap_token_b_amount,
                pool_supply,
                trade_direction,
            )
            .unwrap();

        // perform both separately, updating amounts accordingly
        let (swap_token_a_amount, swap_token_b_amount) = match trade_direction {
            TradeDirection::AtoB => (
                swap_source_amount + results.source_amount_swapped,
                swap_destination_amount - results.destination_amount_swapped,
            ),
            TradeDirection::BtoA => (
                swap_destination_amount - results.destination_amount_swapped,
                swap_source_amount + results.source_amount_swapped,
            ),
        };
        let pool_tokens_from_source = curve
            .deposit_single_token_type(
                source_token_amount - results.source_amount_swapped,
                swap_token_a_amount,
                swap_token_b_amount,
                pool_supply,
                trade_direction,
            )
            .unwrap();
        let pool_tokens_from_destination = curve
            .deposit_single_token_type(
                results.destination_amount_swapped,
                swap_token_a_amount,
                swap_token_b_amount,
                pool_supply + pool_tokens_from_source,
                opposite_direction,
            )
            .unwrap();

        let pool_tokens_total_separate = pool_tokens_from_source + pool_tokens_from_destination;

        // slippage due to rounding or truncation errors
        let epsilon = std::cmp::max(
            1,
            pool_tokens_total_separate * epsilon_in_basis_points / 10000,
        );
        let difference = if pool_tokens_from_one_side >= pool_tokens_total_separate {
            pool_tokens_from_one_side - pool_tokens_total_separate
        } else {
            pool_tokens_total_separate - pool_tokens_from_one_side
        };
        assert!(
            difference <= epsilon,
            "difference expected to be less than {}, actually {}",
            epsilon,
            difference
        );
    }

    /// Test function to check that withdrawing token A is the same as
    /// withdrawing both and swapping one side.
    /// Since calculations use unsigned integers, there will be truncation at
    /// some point, meaning we can't have perfect equality.
    /// We guarantee that the relative error between withdrawing one side and
    /// performing a withdraw plus a swap will be at most some epsilon provided
    /// by the curve. Most curves guarantee accuracy within 0.5%.
    pub fn check_withdraw_token_conversion(
        curve: &dyn CurveCalculator,
        pool_token_amount: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
        trade_direction: TradeDirection,
        epsilon_in_basis_points: u128,
    ) {
        // withdraw the pool tokens
        let withdraw_result = curve
            .pool_tokens_to_trading_tokens(
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
                RoundDirection::Floor,
            )
            .unwrap();

        let new_swap_token_a_amount = swap_token_a_amount - withdraw_result.token_a_amount;
        let new_swap_token_b_amount = swap_token_b_amount - withdraw_result.token_b_amount;

        // swap one side of them
        let source_token_amount = match trade_direction {
            TradeDirection::AtoB => {
                let results = curve
                    .swap_without_fees(
                        withdraw_result.token_a_amount,
                        new_swap_token_a_amount,
                        new_swap_token_b_amount,
                        trade_direction,
                    )
                    .unwrap();
                withdraw_result.token_b_amount + results.destination_amount_swapped
            }
            TradeDirection::BtoA => {
                let results = curve
                    .swap_without_fees(
                        withdraw_result.token_b_amount,
                        new_swap_token_b_amount,
                        new_swap_token_a_amount,
                        trade_direction,
                    )
                    .unwrap();
                withdraw_result.token_a_amount + results.destination_amount_swapped
            }
        };

        // see how many pool tokens it would cost to withdraw one side for the
        // total amount of tokens, should be close!
        let opposite_direction = trade_direction.opposite();
        let pool_token_amount_from_single_side_withdraw = curve
            .withdraw_single_token_type_exact_out(
                source_token_amount,
                swap_token_a_amount,
                swap_token_b_amount,
                pool_token_supply,
                opposite_direction,
                RoundDirection::Ceiling,
            )
            .unwrap();

        // slippage due to rounding or truncation errors
        let epsilon = std::cmp::max(1, pool_token_amount * epsilon_in_basis_points / 10000);
        let difference = if pool_token_amount >= pool_token_amount_from_single_side_withdraw {
            pool_token_amount - pool_token_amount_from_single_side_withdraw
        } else {
            pool_token_amount_from_single_side_withdraw - pool_token_amount
        };
        assert!(
            difference <= epsilon,
            "difference expected to be less than {}, actually {}",
            epsilon,
            difference
        );
    }

    /// Test function checking that a swap never reduces the overall value of
    /// the pool.
    ///
    /// Since curve calculations use unsigned integers, there is potential for
    /// truncation at some point, meaning a potential for value to be lost in
    /// either direction if too much is given to the swapper.
    ///
    /// This test guarantees that the relative change in value will be at most
    /// 1 normalized token, and that the value will never decrease from a trade.
    pub fn check_curve_value_from_swap(
        curve: &dyn CurveCalculator,
        source_token_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
    ) {
        let results = curve
            .swap_without_fees(
                source_token_amount,
                swap_source_amount,
                swap_destination_amount,
                trade_direction,
            )
            .unwrap();

        let (swap_token_a_amount, swap_token_b_amount) = match trade_direction {
            TradeDirection::AtoB => (swap_source_amount, swap_destination_amount),
            TradeDirection::BtoA => (swap_destination_amount, swap_source_amount),
        };
        let previous_value = curve
            .normalized_value(swap_token_a_amount, swap_token_b_amount)
            .unwrap();

        let new_swap_source_amount = swap_source_amount
            .checked_add(results.source_amount_swapped)
            .unwrap();
        let new_swap_destination_amount = swap_destination_amount
            .checked_sub(results.destination_amount_swapped)
            .unwrap();
        let (swap_token_a_amount, swap_token_b_amount) = match trade_direction {
            TradeDirection::AtoB => (new_swap_source_amount, new_swap_destination_amount),
            TradeDirection::BtoA => (new_swap_destination_amount, new_swap_source_amount),
        };

        let new_value = curve
            .normalized_value(swap_token_a_amount, swap_token_b_amount)
            .unwrap();
        assert!(new_value.greater_than_or_equal(&previous_value));

        let epsilon = 1; // Extremely close!
        let difference = new_value
            .checked_sub(&previous_value)
            .unwrap()
            .to_imprecise()
            .unwrap();
        assert!(difference <= epsilon);
    }

    /// Test function checking that a deposit never reduces the value of pool
    /// tokens.
    ///
    /// Since curve calculations use unsigned integers, there is potential for
    /// truncation at some point, meaning a potential for value to be lost if
    /// too much is given to the depositor.
    pub fn check_pool_value_from_deposit(
        curve: &dyn CurveCalculator,
        pool_token_amount: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) {
        let deposit_result = curve
            .pool_tokens_to_trading_tokens(
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
                RoundDirection::Ceiling,
            )
            .unwrap();
        let new_swap_token_a_amount = swap_token_a_amount + deposit_result.token_a_amount;
        let new_swap_token_b_amount = swap_token_b_amount + deposit_result.token_b_amount;
        let new_pool_token_supply = pool_token_supply + pool_token_amount;

        // the following inequality must hold:
        // new_token_a / new_pool_token_supply >= token_a / pool_token_supply
        // which reduces to:
        // new_token_a * pool_token_supply >= token_a * new_pool_token_supply

        // These numbers can be just slightly above u64 after the deposit, which
        // means that their multiplication can be just above the range of u128.
        // For ease of testing, we bump these up to U256.
        let pool_token_supply = U256::from(pool_token_supply);
        let new_pool_token_supply = U256::from(new_pool_token_supply);
        let swap_token_a_amount = U256::from(swap_token_a_amount);
        let new_swap_token_a_amount = U256::from(new_swap_token_a_amount);
        let swap_token_b_amount = U256::from(swap_token_b_amount);
        let new_swap_token_b_amount = U256::from(new_swap_token_b_amount);

        assert!(
            new_swap_token_a_amount * pool_token_supply
                >= swap_token_a_amount * new_pool_token_supply
        );
        assert!(
            new_swap_token_b_amount * pool_token_supply
                >= swap_token_b_amount * new_pool_token_supply
        );
    }

    /// Test function checking that a withdraw never reduces the value of pool
    /// tokens.
    ///
    /// Since curve calculations use unsigned integers, there is potential for
    /// truncation at some point, meaning a potential for value to be lost if
    /// too much is given to the depositor.
    pub fn check_pool_value_from_withdraw(
        curve: &dyn CurveCalculator,
        pool_token_amount: u128,
        pool_token_supply: u128,
        swap_token_a_amount: u128,
        swap_token_b_amount: u128,
    ) {
        let withdraw_result = curve
            .pool_tokens_to_trading_tokens(
                pool_token_amount,
                pool_token_supply,
                swap_token_a_amount,
                swap_token_b_amount,
                RoundDirection::Floor,
            )
            .unwrap();
        let new_swap_token_a_amount = swap_token_a_amount - withdraw_result.token_a_amount;
        let new_swap_token_b_amount = swap_token_b_amount - withdraw_result.token_b_amount;
        let new_pool_token_supply = pool_token_supply - pool_token_amount;

        let value = curve
            .normalized_value(swap_token_a_amount, swap_token_b_amount)
            .unwrap();
        // since we can get rounding issues on the pool value which make it seem that
        // the value per token has gone down, we bump it up by an epsilon of 1
        // to cover all cases
        let new_value = curve
            .normalized_value(new_swap_token_a_amount, new_swap_token_b_amount)
            .unwrap();

        // the following inequality must hold:
        // new_pool_value / new_pool_token_supply >= pool_value / pool_token_supply
        // which can also be written:
        // new_pool_value * pool_token_supply >= pool_value * new_pool_token_supply

        let pool_token_supply = PreciseNumber::new(pool_token_supply).unwrap();
        let new_pool_token_supply = PreciseNumber::new(new_pool_token_supply).unwrap();
        assert!(new_value
            .checked_mul(&pool_token_supply)
            .unwrap()
            .greater_than_or_equal(&value.checked_mul(&new_pool_token_supply).unwrap()));
    }

    prop_compose! {
        pub fn total_and_intermediate(max_value: u64)(total in 1..max_value)
                        (intermediate in 1..total, total in Just(total))
                        -> (u64, u64) {
           (total, intermediate)
       }
    }
}
