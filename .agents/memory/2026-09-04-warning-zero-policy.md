---
name: warning-zero-policy
description: 用户要求编译器零警告零建议——可修复的修复、故意保留的加 #[allow] 注明理由；库一次性格式化已由 rustfmt 统一
metadata:
  type: feedback
---

2026-09-04 用户明确要求：「后续对警告代码需进行修复，如果明确是不需要修复的（比如提前预留、专门就是故意这么写的），则需要做标记，以免编译器提示，需做到编译器无报错、无警告、无任何建议，而通常编译器的建议值得考量。」

**Why:** 编译器/lint 的建议通常有理；跳过修复必须有成文理由（防止「顺手忽略」），但合理保留也要以显式标记换取零噪声。

**How to apply:** 每轮改动后 `cargo clippy --all-targets` 必须零输出再提交；无法/不宜修复的 lint 用 `#[allow(...)]` + 注释写明理由（范例：proxy.rs 的 ForwardResult `large_enum_variant`——Bytes 已引用计数，装箱无收益）。注意 `clippy --fix` 的 let-chains 合并会打乱缩进，跑完必须 `cargo fmt`（本库已全量 rustfmt 一次，提交 682906c，后续保持格式化）。clap 子命令参数过多时用 `#[command(flatten)]` 结构体收编（main.rs 的 ConfigArgs），而非逐字段搬运。相关 [[daemon-model]]。
