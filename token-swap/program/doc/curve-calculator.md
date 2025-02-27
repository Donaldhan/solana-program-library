# `swap` 费用计算和交换机制总结

## 1. 函数概览

```rs

impl SwapCurve {
    /// Subtract fees and calculate how much destination token will be provided
    /// given an amount of source token.
    pub fn swap(
        &self,
        source_amount: u128,
        swap_source_amount: u128,
        swap_destination_amount: u128,
        trade_direction: TradeDirection,
        fees: &Fees,
    ) -> Option<SwapResult> {
        // debit the fee to calculate the amount swapped
        let trade_fee = fees.trading_fee(source_amount)?;
        let owner_fee = fees.owner_trading_fee(source_amount)?;

        let total_fees = trade_fee.checked_add(owner_fee)?;
        let source_amount_less_fees = source_amount.checked_sub(total_fees)?;

        let SwapWithoutFeesResult {
            source_amount_swapped,
            destination_amount_swapped,
        } = self.calculator.swap_without_fees(
            source_amount_less_fees,
            swap_source_amount,
            swap_destination_amount,
            trade_direction,
        )?;

        let source_amount_swapped = source_amount_swapped.checked_add(total_fees)?;
        Some(SwapResult {
            new_swap_source_amount: swap_source_amount.checked_add(source_amount_swapped)?,
            new_swap_destination_amount: swap_destination_amount
                .checked_sub(destination_amount_swapped)?,
            source_amount_swapped,
            destination_amount_swapped,
            trade_fee,
            owner_fee,
        })
    }
    ...
}
```

### `swap` 函数（恒等积x*y=k）


```rust
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
```

### `swap_without_fees` 函数 (稳定币，1:1； token_b_price = init_token_b_amount/ init_token_a_amount)
```rust
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

            let remainder = source_amount_swapped.checked_rem(token_b_price)?;
            if remainder > 0 {
                source_amount_swapped = source_amount.checked_sub(remainder)?;
            }

            (source_amount_swapped, destination_amount_swapped)
        }
    };

    let source_amount_swapped = map_zero_to_none(source_amount_swapped)?;
    let destination_amount_swapped = map_zero_to_none(destination_amount_swapped)?;

    Some(SwapWithoutFeesResult {
        source_amount_swapped,
        destination_amount_swapped,
    })
}
```

### `swap_without_fees` 函数 (偏移量恒等积：token_a * （token_b +offest) = k
```rust
fn swap_without_fees(
    &self,
    source_amount: u128,
    swap_source_amount: u128,
    swap_destination_amount: u128,
    trade_direction: TradeDirection,
) -> Option<SwapWithoutFeesResult> {
    let token_b_offset = self.token_b_offset as u128;
    let swap_source_amount = match trade_direction {
        TradeDirection::AtoB => swap_source_amount,
        TradeDirection::BtoA => swap_source_amount.checked_add(token_b_offset)?,
    };
    let swap_destination_amount = match trade_direction {
        TradeDirection::AtoB => swap_destination_amount.checked_add(token_b_offset)?,
        TradeDirection::BtoA => swap_destination_amount,
    };
    swap(source_amount, swap_source_amount, swap_destination_amount)
}
```

## 2. 核心计算逻辑和公式

### 2.1 `swap` 函数中的计算公式

1. **不变式**：
   $begin:math:display$
   \\text{invariant} = \\text{swap\\_source\\_amount} \\times \\text{swap\\_destination\\_amount}
   $end:math:display$

2. **新的源代币数量**：
   $begin:math:display$
   \\text{new\\_swap\\_source\\_amount} = \\text{swap\\_source\\_amount} + \\text{source\\_amount}
   $end:math:display$

3. **新的目标代币数量**：
   $begin:math:display$
   \\text{new\\_swap\\_destination\\_amount} = \\frac{\\text{invariant}}{\\text{new\\_swap\\_source\\_amount}}
   $end:math:display$

4. **已交换的源代币数量**：
   $begin:math:display$
   \\text{source\\_amount\\_swapped} = \\text{new\\_swap\\_source\\_amount} - \\text{swap\\_source\\_amount}
   $end:math:display$

5. **已交换的目标代币数量**：
   \[
   \text{destination\_amount\_swapped} = \text{swap\_destination\_
      $begin:math:display$
   \\text{destination\\_amount\\_swapped} = \\text{swap\\_destination\\_amount} - \\text{new\\_swap\\_destination\\_amount}
   $end:math:display$

### 2.2 `swap_without_fees` 函数中的计算公式

1. **交易方向：从 B 到 A (`BtoA`)**：
   - 源代币和目标代币的数量计算公式：
     $begin:math:display$
     \\text{source\\_amount\\_swapped} = \\text{source\\_amount}
     $end:math:display$
     $begin:math:display$
     \\text{destination\\_amount\\_swapped} = \\text{source\\_amount} \\times \\text{token\\_b\\_price}
     $end:math:display$

2. **交易方向：从 A 到 B (`AtoB`)**：
   - 目标代币 B 的数量是源代币 A 除以目标代币 B 的价格：
     $begin:math:display$
     \\text{destination\\_amount\\_swapped} = \\frac{\\text{source\\_amount}}{\\text{token\\_b\\_price}}
     $end:math:display$
   - 源代币 A 的数量会向下取整，去掉余数：
     $begin:math:display$
     \\text{source\\_amount\\_swapped} = \\text{source\\_amount} - \\text{remainder}
     $end:math:display$
     其中，`remainder` 为源代币数量除以目标代币价格后的余数：
     $begin:math:display$
     \\text{remainder} = \\text{source\\_amount} \\mod \\text{token\\_b\\_price}
     $end:math:display$

#### 2.3 价格与偏移量调整

- **`token_b_offset`**：在不同的交易方向（`AtoB` 或 `BtoA`）下，`token_b_offset` 用于调整源代币和目标代币的数量，确保交换过程的合理性。

---

## 3. 总结

1. **`swap` 函数**：
   - 该函数通过不变式计算源代币和目标代币数量，确保交换过程中的代币数量计算正确。核心公式包括乘法、加法、除法，以及向上取整的操作。
   
2. **价格与兑换比率**：
   - `token_b_price` 决定了源代币和目标代币之间的兑换比率，在计算过程中影响目标代币的数量。

3. **余数处理**：
   - 在从 A 到 B 进行兑换时，若源代币 A 的数量无法完全兑换为目标代币 B（即存在余数），则会通过向下取整处理，去掉余数，避免兑换出超出预期的目标代币数量。

4. **偏移量调整**：
   - 在交易方向调整时，通过 `token_b_offset` 调整源代币和目标代币的数量，确保根据不同的交易需求合理处理兑换量。

---

## 参考公式总结

1. **不变式**：
   $begin:math:display$
   \\text{invariant} = \\text{swap\\_source\\_amount} \\times \\text{swap\\_destination\\_amount}
   $end:math:display$

2. **新的源代币数量**：
   $begin:math:display$
   \\text{new\\_swap\\_source\\_amount} = \\text{swap\\_source\\_amount} + \\text{source\\_amount}
   $end:math:display$

3. **新的目标代币数量**：
   $begin:math:display$
   \\text{new\\_swap\\_destination\\_amount} = \\frac{\\text{invariant}}{\\text{new\\_swap\\_source\\_amount}}
   $end:math:display$

4. **已交换的源代币数量**：
   $begin:math:display$
   \\text{source\\_amount\\_swapped} = \\text{new\\_swap\\_source\\_amount} - \\text{swap\\_source\\_amount}
   $end:math:display$

5. **已交换的目标代币数量**：
   $begin:math:display$
   \\text{destination\\_amount\\_swapped} = \\text{swap\\_destination\\_amount} - \\text{new\\_swap\\_destination\\_amount}
   $end:math:display$

6. **从 A 到 B 的兑换公式**：
   $begin:math:display$
   \\text{destination\\_amount\\_swapped} = \\frac{\\text{source\\_amount}}{\\text{token\\_b\\_price}}
   $end:math:display$

7. **源代币 A 的数量向下取整公式**：
   $begin:math:display$
   \\text{source\\_amount\\_swapped} = \\text{source\\_amount} - \\text{remainder}
   $end:math:display$
   其中，`remainder` 为余数：
   $begin:math:display$
   \\text{remainder} = \\text{source\\_amount} \\mod \\text{token\\_b\\_price}
   $end:math:display$

---

以上是对 `swap` 函数和 `swap_without_fees` 函数的详细总结，涵盖了它们的核心计算逻辑、公式以及如何根据交易方向调整代币数量。