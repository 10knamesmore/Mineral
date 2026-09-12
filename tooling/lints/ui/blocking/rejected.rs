//! 验证导入别名、关联调用、异步块与异步闭包中的阻塞诊断。
#![allow(dead_code)]
#![deny(mineral_blocking_in_async)]
use std::{sync::mpsc::Receiver, thread::sleep as pause, time::Duration};
/// 覆盖直接调用及嵌套异步执行体中的七处阻塞操作。
async fn rejected(receiver: Receiver<()>) {
    pause(Duration::ZERO);
    std::thread::sleep(Duration::ZERO);
    let _ = receiver.recv();
    let _ = Receiver::recv(&receiver);
    let _ = std::fs::read("unused");
    let _ = async { pause(Duration::ZERO) };
    let _ = async || pause(Duration::ZERO);
}
/// UI 编译夹具入口。
fn main() {}
