#![forbid(unsafe_code)]

//! loomos 设备核心（architecture/06）。
//!
//! Device Core 只统一身份、权限、生命周期、资源所有权、操作跟踪、遥测、故障报告；
//! 领域操作留给 Domain（不设计只含 turn_on/turn_off 的万能 Device Trait）。
//!
//! Operation 状态（06 §6.4）：Accepted → Queued → Executing →
//! Succeeded / Failed / Cancelled / Expired / **Unknown**。
//! 取消语义由 Backend 明确：排队时可取消 ≠ 执行中可取消；Unknown 不得报成 Failed。

pub mod core;
pub mod http;
pub mod mock;

pub use core::{
    DeviceCapability, DeviceDescriptor, DeviceError, DeviceHealth, DeviceLifecycle, DeviceRegistry,
    HealthState, OperationRecord, OperationState, ProviderId,
};
pub use http::{HttpBoardAdapter, MiniHttpError, mini_http_request, mini_http_server};
pub use mock::MockHomeProvider;
