//! 输入诊断纯数据类型 + InputScope 掩码判定（无 I/O，可单测）。

/// InputScope 位：与 C++ kScopeBitPassword / Go 端一致。
const IS_PASSWORD_BIT: u64 = 1 << 31;
const IS_NUMERIC_PASSWORD_BIT: u64 = 1 << 63;
/// `IS_PRIVATE`（枚举值 61）。Chromium 密码框的指纹之一：context 级禁用 + 宿主原始 scope 带此位。
const IS_PRIVATE_BIT: u64 = 1 << 61;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum InputDiagReason {
    #[default]
    None,
    CompartmentDisabled,
    InputScopePassword,
    NumericPassword,
    /// 焦点 context 上的 `KEYBOARD_DISABLED`（context 级）置位，而宿主原始 InputScope **没有**密码位。
    /// 与 `InputScopePassword` 是两回事：前者是宿主明说「这是密码框」，后者只是宿主说「此处禁用输入法」
    /// （Chromium 密码框会这么置，Gecko 系无输入焦点时也会——t197）。抑制行为相同，展示与提示语不同。
    ContextDisabled,
    /// context 级禁用 + 宿主原始 scope 带 `IS_PRIVATE`：Chromium 系网页密码框的指纹
    /// （2026-09-30 靶机实测：Chrome 密码框 rawScope=0x2000000000000000 且 ctxKbdDisabled=1；
    /// Zen 无输入焦点态 rawScope=0x1 且 ctxKbdDisabled=1；Chrome 无痕普通框只有 IS_PRIVATE、
    /// 不置 context 禁用）。宿主没有明说「密码」，但两个信号同时出现足以认定，展示按密码框处理。
    ContextPassword,
}

/// DLL 上报的 `reason` 字节里「context 级禁用、宿主未报密码 scope」这一档
/// （对齐 C++ `ComputeInputReason` 的取值 4）。0..=3 沿用旧语义，旧 DLL 永远不会发 4。
pub const REPORT_REASON_CONTEXT_DISABLED: u8 = 4;

/// 判定禁用原因。compartment（DLL 已放行所有键）优先级最高。
pub fn reason_from(disabled: bool, mask: u64) -> InputDiagReason {
    if disabled {
        return InputDiagReason::CompartmentDisabled;
    }
    if mask & IS_NUMERIC_PASSWORD_BIT != 0 {
        return InputDiagReason::NumericPassword;
    }
    if mask & IS_PASSWORD_BIT != 0 {
        return InputDiagReason::InputScopePassword;
    }
    InputDiagReason::None
}

/// 结合 DLL 上报的 `reason` 字节推导展示原因。
///
/// 为什么不能只看 mask：DLL 把 context 级 `KEYBOARD_DISABLED` **折进** mask 的 IS_PASSWORD 位
/// （抑制门控要靠它，见 C++ `OnSetFocus`），于是仅凭 mask 分不出「宿主报了密码」与「宿主只是禁用」。
/// 折位之前的区分只有 DLL 知道，故由它用 `reason` 字节带过来。旧 DLL 不带（永远 0..=3）时
/// 退回按 mask 推导——展示会把 context 级禁用说成「密码」，但那是旧行为，不比以前更差。
pub fn reason_from_report(disabled: bool, reason_byte: u8, mask: u64) -> InputDiagReason {
    if !disabled && reason_byte == REPORT_REASON_CONTEXT_DISABLED && is_password_scope(mask) {
        // 折位摘掉后才是宿主原始 scope：IS_PRIVATE 只看它，不看折进去的位。
        return if mask & IS_PRIVATE_BIT != 0 {
            InputDiagReason::ContextPassword
        } else {
            InputDiagReason::ContextDisabled
        };
    }
    reason_from(disabled, mask)
}

/// 展示用的 InputScope：context 级禁用那档要把折进去的 IS_PASSWORD 位摘掉，还原**宿主原始报的值**。
pub fn display_mask(reason: InputDiagReason, mask: u64) -> u64 {
    if matches!(
        reason,
        InputDiagReason::ContextDisabled | InputDiagReason::ContextPassword
    ) {
        mask & !IS_PASSWORD_BIT
    } else {
        mask
    }
}

/// mask 是否命中密码/数字密码位（用于抑制策略）。
pub fn is_password_scope(mask: u64) -> bool {
    mask & (IS_PASSWORD_BIT | IS_NUMERIC_PASSWORD_BIT) != 0
}

pub fn reason_label(r: InputDiagReason) -> &'static str {
    match r {
        InputDiagReason::None => "无",
        InputDiagReason::CompartmentDisabled => "线程级禁用",
        InputDiagReason::InputScopePassword => "宿主报密码",
        InputDiagReason::NumericPassword => "宿主报数字密码",
        InputDiagReason::ContextDisabled => "context 禁用",
        InputDiagReason::ContextPassword => "密码框(context 禁用+私密)",
    }
}

#[derive(Clone, Debug, Default)]
pub struct InputDiagState {
    pub pid: u32,
    pub process_name: String,
    pub disabled: bool,
    pub reason: InputDiagReason,
    pub mask: u64,
}

/// 窗口 / TSF 上下文诊断快照定义在 wind-ui-types（表现层协议 crate，
/// 协调器与渲染端共同的依赖）；这里 re-export 一次，让本模块仍是
/// "输入诊断数据类型"的单一入口。
pub use wind_ui_types::WindowDiagView;

#[cfg(test)]
mod tests {
    use super::*;

    const IS_PASSWORD: u64 = 1 << 31;
    const IS_NUMERIC_PASSWORD: u64 = 1 << 63;

    #[test]
    fn reason_none_when_clean() {
        assert_eq!(reason_from(false, 0), InputDiagReason::None);
    }

    #[test]
    fn compartment_takes_precedence_over_mask() {
        // disabled=true 一律 CompartmentDisabled，即便 mask 有密码位
        assert_eq!(
            reason_from(true, IS_PASSWORD),
            InputDiagReason::CompartmentDisabled
        );
    }

    #[test]
    fn password_and_numeric_from_mask() {
        assert_eq!(
            reason_from(false, IS_PASSWORD),
            InputDiagReason::InputScopePassword
        );
        assert_eq!(
            reason_from(false, IS_NUMERIC_PASSWORD),
            InputDiagReason::NumericPassword
        );
    }

    #[test]
    fn is_password_scope_covers_both_bits() {
        assert!(is_password_scope(IS_PASSWORD));
        assert!(is_password_scope(IS_NUMERIC_PASSWORD));
        assert!(!is_password_scope(0));
    }

    #[test]
    fn report_reason_separates_context_disabled_from_host_password() {
        // DLL 折位后 mask 带 bit31，reason 字节 4：宿主并没报密码。
        let folded = 0x8000_0001;
        assert_eq!(
            reason_from_report(false, REPORT_REASON_CONTEXT_DISABLED, folded),
            InputDiagReason::ContextDisabled
        );
        // 宿主真报了 IS_PASSWORD：DLL 发 reason=2，仍是密码。
        assert_eq!(
            reason_from_report(false, 2, folded),
            InputDiagReason::InputScopePassword
        );
        // 旧 DLL 永远不发 4：退回按 mask 推导。
        assert_eq!(
            reason_from_report(false, 0, folded),
            InputDiagReason::InputScopePassword
        );
        // 线程级禁用优先级最高，不被 4 抢走。
        assert_eq!(
            reason_from_report(true, REPORT_REASON_CONTEXT_DISABLED, folded),
            InputDiagReason::CompartmentDisabled
        );
        // Chromium 密码框：context 禁用 + 宿主原始 scope 带 IS_PRIVATE ⇒ 按密码框展示。
        assert_eq!(
            reason_from_report(
                false,
                REPORT_REASON_CONTEXT_DISABLED,
                0x8000_0000 | (1 << 61)
            ),
            InputDiagReason::ContextPassword
        );
        // 谎报 4 但 mask 里没有密码位：不采信（抑制与展示不应自相矛盾）。
        assert_eq!(
            reason_from_report(false, REPORT_REASON_CONTEXT_DISABLED, 0x1),
            InputDiagReason::None
        );
    }

    #[test]
    fn display_mask_strips_folded_password_bit_only_for_context_disabled() {
        assert_eq!(
            display_mask(InputDiagReason::ContextDisabled, 0x8000_0001),
            0x1
        );
        assert_eq!(
            display_mask(InputDiagReason::ContextPassword, 0x8000_0000 | (1 << 61)),
            1 << 61
        );
        assert_eq!(
            display_mask(InputDiagReason::InputScopePassword, 0x8000_0001),
            0x8000_0001
        );
    }
}
