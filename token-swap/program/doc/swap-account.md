# Solana Token Swap 账户关系解析

## 1. 主要账户角色
在 `process_swap` 交易中，涉及到多个账户，主要包括：
- **用户账户**（存放用户的代币）
- **交换池账户**（存放流动性池的代币）
- **流动性池 mint 账户**（用于铸造 LP 代币）
- **授权账户（authority）**（控制交换池的权限）
- **代币 mint 账户**（用于生成和管理代币）

---

## 2. 各个账户的作用及关系

| 账户名称                     | 账户类型     | 作用描述 |
|----------------------------|------------|------------------------------------------------------------|
| `source_info`              | 账户       | 用户的输入代币账户（来源账户） |
| `destination_info`         | 账户       | 用户的输出代币账户（目标账户） |
| `swap_source_info`         | 账户       | 交换池中存放输入代币的账户 |
| `swap_destination_info`    | 账户       | 交换池中存放输出代币的账户 |
| `swap_info`                | 账户       | 交换池管理账户，包含 `token_swap` 信息 |
| `authority_info`           | 账户       | 交换池的权限账户，必须匹配 `swap_info` 生成的 authority |
| `pool_mint_info`           | Mint 账户  | 交换池 LP 代币的 Mint 账户 |
| `source_token_mint_info`   | Mint 账户  | 源代币的 Mint 账户 |
| `destination_token_mint_info` | Mint 账户  | 目标代币的 Mint 账户 |
| `user_transfer_authority_info` | 账户   | 用户代币转账授权账户 |
| `pool_fee_account_info`    | 账户       | 交换池手续费账户 |

---

## 3. 账户之间的关系（核心逻辑）

### (1) 用户与流动性池
用户希望将 `amount_in` 个 `token_A` 兑换为 `token_B`：
- 用户将 `token_A` 发送到 **交换池 `swap_source_info`**。
- 交换池通过 **预设的交易曲线** 计算 `token_B` 兑换数量。
- 交换池将 `token_B` 发送到 **用户的 `destination_info`** 账户。

```rust
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
```

```rust
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
```

---

### (2) Mint 账户的作用
- `source_token_mint_info`：用于检查 `source_info` 账户中存储的代币类型。
- `destination_token_mint_info`：用于检查 `destination_info` 账户存储的目标代币类型。
- `pool_mint_info`：如果是流动性提供者（LP）存取款，`pool_mint_info` 负责生成 LP 代币。

---

### (3) Authority（权限控制）
Solana 采用 PDA（Program Derived Address）机制来授权特定合约对账户的管理。

```rust
if *authority_info.key != Self::authority_id(program_id, swap_info.key, token_swap.bump_seed())? {
    return Err(SwapError::InvalidProgramAddress.into());
}
```

该 `authority_info` 主要用于：
1. 允许合约管理 `swap_source_info` 和 `swap_destination_info` 的资金。
2. 允许合约调用 `mint_to` 和 `burn` 以处理流动性提供者的 LP 代币。

---

### (4) Swap 计算（曲线模型）
交换过程中，系统根据 `swap_curve()` 计算 `token_A` 到 `token_B` 的转换比例：

```rust
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
```

---

### (5) 处理 Swap 费用
交换过程中，有两种费用：
- **转账费用**：部分代币在转账时收取，如 `TransferFeeConfig` 机制。
- **协议费用**：交易所需要抽取手续费，例如：

```rust
if result.owner_fee > 0 {
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
}
```

---

## 4. 账户关系示意图
```
  [ 用户账户 ]                  [ 交换池账户 ]
     source_info -----> swap_source_info
                        swap_destination_info -----> destination_info
                                    |
                                    |
                            [ 流动性池 Mint ]
                                  |
                           pool_mint_info
                                  |
                       [ 费用池 pool_fee_account_info ]
```
1. 用户将 `token_A` 从 `source_info` 账户转入 `swap_source_info`。
2. 交换池计算 `token_B` 兑换比例，并从 `swap_destination_info` 转给用户 `destination_info`。
3. 交易费用可能会存入 `pool_fee_account_info`，协议或流动性提供者可以领取。

---

## 5. 总结
- `source_info`（用户存放的源代币）转账到 `swap_source_info`（池子）。
- `swap_destination_info`（池子）再转账 `destination_info`（用户存放的目标代币）。
- `authority_info` 负责控制 `swap_source_info` 和 `swap_destination_info` 账户的管理。
- `mint` 账户负责确定代币类型，同时用于计算费用或 LP 代币的铸造。
- `pool_fee_account_info` 存储协议或流动性提供者的费用。

这个设计保证了交易的去中心化、自动执行和安全性，同时也允许协议抽取费用来激励流动性提供者。

