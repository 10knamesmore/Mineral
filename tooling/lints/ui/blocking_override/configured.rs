//! 验证显式配置完整替换默认阻塞清单。
#![allow(dead_code)]
/// 由配置指定的自定义阻塞调用。
fn custom_wait() {}
/// 默认线程计时器被移出清单，仅自定义调用产生诊断。
async fn handler() {
    std::thread::sleep(std::time::Duration::ZERO);
    custom_wait();
}

/// UI 编译夹具入口。
fn main() {}
