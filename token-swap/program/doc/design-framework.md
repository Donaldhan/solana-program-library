processor.rs
```rs
 /// Processes an [Instruction](enum.Instruction.html).  处理所有swap相关的指令
    pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], input: &[u8]) -> ProgramResult {
        Self::process_with_constraints(program_id, accounts, input, &SWAP_CONSTRAINTS)
    }
```



