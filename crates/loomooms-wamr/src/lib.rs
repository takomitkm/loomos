//! WAMR Runtime Host（ADR-011 P1）。
//!
//! LoomOS 统一采用 Wasm AOT 执行组件（ADR-011）。本 crate 是 Native Host
//! 内嵌的 WAMR 运行时包装：模块加载/卸载、实例管理、内存/栈预算、Host API 注册。
//!
//! **两档模式**：
//! - `wamr-link` feature（生产）：WAMR C API 静态链接（需 IWASM_LIB_DIR）；
//! - 默认（stub）：所有执行函数返回 `WamrError::NotLinked`——crate 始终可编译。
//!
//! **Host ABI v1**（ADR-011 P2）：
//! - Core Wasm + WASI P1 标准导入 + 自定义 `loomos.*` Host 函数（版本化）；
//! - Guest 不允许直接取得物理 DMA 地址或厂商 Runtime 指针；
//! - AI 推理（ONNX / sherpa / NPU）留在 Native Host，通过 Host ABI 暴露。

#[cfg(feature = "wamr-link")]
#[cfg(target_os = "linux")]
pub mod ffi;

pub mod error;
pub mod host_abi;
pub mod instance;
pub mod module;
pub mod runtime;

pub use error::{WamrError, WamrResult};
pub use host_abi::{HOST_ABI_V1, HOST_ABI_VERSION, HOST_NAMESPACE};
pub use instance::{InstanceHandle, InstanceManager, InstanceSpec};
pub use module::{ModuleHandle, ModuleInfo, ModuleManager};
pub use runtime::WamrRuntime;
