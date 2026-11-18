//! normos 推理编排（architecture/05）。
//!
//! 边界三条：
//! 1. 复用厂商编译器与 Runtime，不重写其内部执行系统（原型阶段：后端为 mock / 回声档，
//!    真 sherpa-onnx / 厂商 NPU Runtime 走同一 `Backend` trait 的适配器）；
//! 2. 系统级编排（排队、准入、公平性、恢复）属于本 crate；
//! 3. 不假定厂商 Runtime 的并发能力——按同步阻塞、无 cancel 建模（05 §5.4）。
//!
//! 契约要点：
//! - 排队中的请求可取消（在进入 forward 之前）；
//! - 已进入 forward 的请求不能承诺及时取消 → `timeout_unknown`；
//! - 超时返回 ≠ 硬件已停止访问 Buffer → 未确认完成前不得释放 Buffer；
//! - 状态只报 accepted/queued/running/completed/failed/unknown，不得把 unknown 报成 failed。

pub mod buffer;
pub mod orchestrator;

pub use buffer::{
    Buffer, BufferError, CpuOwned, CpuPhysicalAddress, CpuVirtualAddress, DeviceComplete,
    DeviceDmaAddress, DeviceOwned, DeviceState, SyncForCpu, SyncForDevice,
};
pub use orchestrator::{
    AdmissionError, Backend, BackendError, BackendOutput, FailureClass, InferenceRequest,
    InferenceState, ModelProfile, Orchestrator, SubmitTicket,
};
