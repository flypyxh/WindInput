//! `compat.*` 路由：必须走到 `wind_config::compat_admin`，而不是落进 `unknown method`。
//!
//! 只测「路由通了 + 坏请求是 Err」。规则的合并 / 写时复制 / 导入导出语义在
//! `wind-config` 的 `compat_admin` 单测里，写入后的重载在 `wind-coordinator` 的
//! `compat_reload_tests` 里——这里刻意不碰真实用户目录的写入路径。
use serde_json::json;
use wind_config::Config;
use wind_coordinator::Coordinator;
use wind_webdata::WebDataRpc;

/// `compat.schema` 是纯函数（不读目录），可以放心在测试里直接调。
#[test]
fn compat_schema_is_routed_and_lists_fields() {
    let c = Coordinator::new_headless(Config::default(), None);
    let v = c.web_data_rpc("compat.schema", &json!({})).unwrap();
    let fields = v["fields"].as_array().expect("fields 应是数组");
    assert!(
        fields.iter().any(|f| f["key"] == "stale_probe_guard"),
        "元数据应含 stale_probe_guard"
    );
    assert!(v["groups"].is_array());
}

/// 坏请求只会得到 Err，不会 panic，也不会被当成别的命名空间的方法吞掉。
#[test]
fn compat_bad_requests_come_back_as_errors() {
    let c = Coordinator::new_headless(Config::default(), None);
    assert!(
        c.web_data_rpc("compat.list", &json!({"section": "x"}))
            .is_err()
    );
    assert!(c.web_data_rpc("compat.bogus", &json!({})).is_err());
}
