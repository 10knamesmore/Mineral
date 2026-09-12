//! 验证异步执行体识别、完整配置覆盖与无效配置诊断。

/// 默认清单拒绝异步阻塞调用，允许同步执行体及不同定义的同名方法。
#[test]
fn blocking_calls() {
    dylint_testing::ui::Test::src_base(env!("CARGO_PKG_NAME"), "ui/blocking")
        .rustc_flags(["--edition=2024"])
        .run();
}

/// 显式清单替换默认清单，只拒绝配置中的自定义函数。
#[test]
fn configured_calls_replace_defaults() {
    dylint_testing::ui::Test::src_base(env!("CARGO_PKG_NAME"), "ui/blocking_override")
        .rustc_flags(["--edition=2024"])
        .dylint_toml(
            "[mineral_blocking_in_async]\ncalls = [{ path = \"configured::custom_wait\", reason = \"configured blocking operation\" }]",
        )
        .run();
}

/// 无效配置即使没有待检查的业务调用也必须阻止编译。
#[test]
fn invalid_configuration_is_an_error() {
    dylint_testing::ui::Test::src_base(env!("CARGO_PKG_NAME"), "ui/invalid_config")
        .rustc_flags(["--edition=2024"])
        .dylint_toml("[mineral_blocking_in_async]\ncallz = []")
        .run();
}
