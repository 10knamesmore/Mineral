//! 验证同步执行体与其他定义的同名调用不触发阻塞诊断。
#![allow(dead_code)]
#![deny(mineral_blocking_in_async)]

use std::{future::ready, time::Duration};

/// 同步函数可以使用线程计时器。
fn synchronous_sleep() {
    std::thread::sleep(Duration::ZERO);
}

/// 与标准库同名但定义不同的非阻塞函数。
fn sleep(_duration: Duration) {}

/// 与标准库接收器使用相同方法名的独立类型。
struct OtherReceiver;

impl OtherReceiver {
    /// 模拟无需等待的接收操作。
    fn recv(&self) {}
}

/// 异步执行体中的同步闭包与嵌套同步函数具有各自的执行体。
async fn allowed(receiver: OtherReceiver) {
    sleep(Duration::ZERO);
    receiver.recv();

    let synchronous_closure = || std::thread::sleep(Duration::ZERO);
    synchronous_closure();

    /// 嵌套同步函数的调用不继承外层异步上下文。
    fn nested_synchronous_sleep() {
        std::thread::sleep(Duration::ZERO);
    }
    nested_synchronous_sleep();

    ready(()).await;
}

/// UI 编译夹具入口。
fn main() {}
