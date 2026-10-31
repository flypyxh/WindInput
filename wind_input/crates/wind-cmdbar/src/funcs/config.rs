//! 配置读写动作（对照 Go funcs/config_funcs.go）。`config.get` 为纯函数（依赖配置状态，
//! 非确定），`config.set` / `config.toggle` 为副作用。经
//! [`ConfigService`](crate::services::ConfigService)，key 为 YAML 路径。

use super::action::check_enum;
use super::func_specs;
use super::util::{runtime_err, services};
use crate::context::EvalContext;
use crate::error::{CmdbarError, Result};
use crate::registry::FuncSpec;

/// `config.set` 的 `toast` 取值白名单。没有 `wind.cli` 那样的 `error` 档：
/// 写回失败的提示由 cmdbar 的错误通路负责（与 `toast` 无关地照常弹），这里只管
/// **成功**那条「设置已更新」。
const SET_TOAST_MODES: &[&str] = &["on", "off"];

pub fn specs() -> Vec<FuncSpec> {
    func_specs! {
        "config.get"   : Config (1, 1) pure   => fn_get,    "读取配置项当前值; key 为 YAML 路径 (如 ui.candidate.layout)", "config.get(\"ui.theme.style\")";
        "config.set"   : Config (2, 2) effect => fn_set,    "设置配置项并持久化; key 为 YAML 路径, value 为字符串", "config.set(\"ui.theme.style\", \"dark\")"
            named(fn_set_named,
                  "toast" = "写回并热重载成功后的提示: on(默认,弹「设置已更新」)/off; 一条命令里后面还有自己的提示(如 wind.cli(…, ok=…))时设 off");
        "config.toggle": Config (1, 1) effect => fn_toggle, "枚举循环切换 / bool 翻转, 持久化并返回新值", "config.toggle(\"ui.theme.style\")";
    }
}

fn fn_get(ctx: &dyn EvalContext, args: &[String]) -> Result<String> {
    let s = services("config.get", ctx)?;
    let config = s
        .config
        .as_ref()
        .ok_or_else(|| CmdbarError::service("config.get"))?;
    config
        .get(&args[0])
        .map_err(|e| runtime_err("config.get", e))
}

fn fn_set(ctx: &dyn EvalContext, args: &[String]) -> Result<String> {
    fn_set_named(ctx, args, &[])
}

/// `config.set(key, value, toast="on|off")`。
fn fn_set_named(
    ctx: &dyn EvalContext,
    args: &[String],
    named: &[(String, String)],
) -> Result<String> {
    let s = services("config.set", ctx)?;
    let config = s
        .config
        .as_ref()
        .ok_or_else(|| CmdbarError::service("config.set"))?;
    let toast = named_val(named, "toast");
    check_enum("config.set", "toast", toast, SET_TOAST_MODES)?;
    config
        .set_quiet(&args[0], &args[1], toast == "off")
        .map_err(|e| runtime_err("config.set", e))?;
    Ok(String::new())
}

fn fn_toggle(ctx: &dyn EvalContext, args: &[String]) -> Result<String> {
    let s = services("config.toggle", ctx)?;
    let config = s
        .config
        .as_ref()
        .ok_or_else(|| CmdbarError::service("config.toggle"))?;
    config
        .toggle(&args[0])
        .map_err(|e| runtime_err("config.toggle", e))
}

/// 取具名参数值；名字白名单已在 [`crate::eval`] 的 `call_func` 校验过，此处只取。
fn named_val<'a>(named: &'a [(String, String)], key: &str) -> &'a str {
    named
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::MemoryContext;
    use crate::services::Services;
    use std::sync::Mutex;

    /// 记录 set / set_quiet 的入参，其余操作返回失败（本组用例不走到）。
    #[derive(Default)]
    struct RecConfig {
        calls: Mutex<Vec<(String, String, bool)>>,
    }
    impl crate::services::ConfigService for RecConfig {
        fn get(&self, _key: &str) -> anyhow::Result<String> {
            unreachable!()
        }
        fn set(&self, key: &str, value: &str) -> anyhow::Result<()> {
            self.record(key, value, false)
        }
        fn set_quiet(&self, key: &str, value: &str, quiet: bool) -> anyhow::Result<()> {
            self.record(key, value, quiet)
        }
        fn toggle(&self, _key: &str) -> anyhow::Result<String> {
            unreachable!()
        }
    }
    impl RecConfig {
        fn record(&self, key: &str, value: &str, quiet: bool) -> anyhow::Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push((key.to_string(), value.to_string(), quiet));
            Ok(())
        }
    }

    fn ctx_with(rec: &std::sync::Arc<RecConfig>) -> MemoryContext {
        let mut svc = Services::new();
        svc.config = Some(rec.clone());
        MemoryContext::new().with_services(svc)
    }

    #[test]
    fn set_defaults_to_loud() {
        let rec = std::sync::Arc::new(RecConfig::default());
        let ctx = ctx_with(&rec);
        fn_set_named(&ctx, &["ui.theme.style".into(), "dark".into()], &[]).unwrap();
        assert_eq!(
            rec.calls.lock().unwrap().as_slice(),
            &[("ui.theme.style".into(), "dark".into(), false)]
        );
    }

    #[test]
    fn set_toast_off_requests_quiet() {
        let rec = std::sync::Arc::new(RecConfig::default());
        let ctx = ctx_with(&rec);
        fn_set_named(
            &ctx,
            &["ui.theme.style".into(), "dark".into()],
            &[("toast".into(), "off".into())],
        )
        .unwrap();
        assert_eq!(
            rec.calls.lock().unwrap().as_slice(),
            &[("ui.theme.style".into(), "dark".into(), true)]
        );
    }

    #[test]
    fn set_rejects_unknown_toast_mode() {
        let rec = std::sync::Arc::new(RecConfig::default());
        let ctx = ctx_with(&rec);
        let err = fn_set_named(
            &ctx,
            &["ui.theme.style".into(), "dark".into()],
            &[("toast".into(), "error".into())],
        )
        .unwrap_err();
        assert!(err.to_string().contains("toast"), "实际: {err}");
        assert!(rec.calls.lock().unwrap().is_empty(), "校验失败不得写回");
    }

    /// 具名参数白名单写进 spec，写错的参数名在求值期就该报错（eval 层守卫），
    /// 这里只钉 spec 声明本身。
    #[test]
    fn set_declares_toast_named_param() {
        let spec = specs()
            .into_iter()
            .find(|s| s.name == "config.set")
            .unwrap();
        assert!(spec.accepts_named("toast"));
        assert!(!spec.accepts_named("ok"));
        assert!(spec.eval_named.is_some());
    }

    /// 端到端：词条里写 `config.set(…, toast="off")` 要能解析、求值并把 quiet 传到服务。
    #[test]
    fn phrase_syntax_toast_off_reaches_service() {
        use crate::phrase;
        let rec = std::sync::Arc::new(RecConfig::default());
        let ctx = ctx_with(&rec);
        let reg = crate::registry::Registry::full();
        let r = phrase::evaluate_phrase(
            r#"$CC("[开关]", config.set("ui.theme.style", "dark", toast="off"))"#,
            &ctx,
            &reg,
        )
        .unwrap();
        match r {
            phrase::PhraseEval::Single {
                actions: a,
                on_error,
                ..
            } => {
                phrase::run_actions(&a, &ctx, &reg, on_error);
            }
            _ => panic!(),
        }
        assert_eq!(
            rec.calls.lock().unwrap().as_slice(),
            &[("ui.theme.style".into(), "dark".into(), true)]
        );
    }

    /// 默认（不带 toast）走完整管线同样通，语义保持改动前。
    #[test]
    fn phrase_syntax_without_toast_still_loud() {
        use crate::phrase;
        let rec = std::sync::Arc::new(RecConfig::default());
        let ctx = ctx_with(&rec);
        let reg = crate::registry::Registry::full();
        let r = phrase::evaluate_phrase(
            r#"$CC("[开关]", config.set("ui.theme.style", "dark"))"#,
            &ctx,
            &reg,
        )
        .unwrap();
        match r {
            phrase::PhraseEval::Single {
                actions: a,
                on_error,
                ..
            } => {
                phrase::run_actions(&a, &ctx, &reg, on_error);
            }
            _ => panic!(),
        }
        assert_eq!(
            rec.calls.lock().unwrap().as_slice(),
            &[("ui.theme.style".into(), "dark".into(), false)]
        );
    }
}
