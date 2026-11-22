//! WAMR Runtime Host（ADR-011 v1.1 P1）——**权威组件所在的 Native Host 内嵌执行层**。
//!
//! 职责边界（ADR-011 v1.1 §主权边界）：
//! - 本 crate 执行 Wasm 业务组件（load/unload/instantiate/invoke）；
//! - **权限权威永远不在本 crate 的 Guest 侧**——CapabilityStore / ResourceLease /
//!   Supervisor / LifecycleManager 留在 `loomos-runtime`（Native Host），
//!   Guest 只经 Host ABI（P2）以受限客户端访问；
//! - raw WAMR FFI 全部在 `loomos-wamr-sys`（第三声明式可信边界）；
//!   本 crate `#![forbid(unsafe_code)]`，只持有不透明句柄。
//!
//! **两档模式**：
//! - `wamr-link` feature（生产）：WAMR C API 静态链接（IWASM_LIB_DIR；
//!   `nix build .#loomos-wamr` 产出）；
//! - 默认（stub）：执行类操作返回 `WamrError::NotLinked`——crate 始终可编译，
//!   簿记/格式校验/兼容键/预算逻辑全部可测。
//!
//! 关键语义（来源：loomos-nlu-bench CI/QEMU 实测，checksum 全一致）：
//! - `wasm_runtime_call_wasm` 返回值在 **argv[0]**；
//! - 模块缓冲与模块同生命周期（WAMR 可能引用其内容）；
//! - AOT 魔数 `\0aot` + 格式版本 6（core/config.h @06f6c8c）。

#![forbid(unsafe_code)]

pub mod aot_compat;
pub mod error;
pub mod host_abi;
pub mod instance;
pub mod module;
pub mod package;
pub mod runtime;
pub mod sha256;

pub use aot_compat::AotCompatKey;
pub use error::{WamrError, WamrResult};
pub use host_abi::leaked_symbol_table;
pub use host_abi::{HOST_ABI_VERSION, HOST_NAMESPACE};
pub use instance::{InstanceHandle, InstanceManager, InstanceSpec, Reservation};
pub use module::{ExecMode, LoadedModule, ModuleFormat, ModuleManager, detect_format};
pub use package::{ComponentPackage, PackageManifest};
pub use runtime::{RuntimeConfig, RuntimeHost};
pub use sha256::sha256_hex;
