//! 推理编排：单 Owner、排队/准入、同步阻塞后端契约（architecture/05）。
//!
//! 关键契约（05 §5.4）：
//! - 排队中的请求可取消（在进入 forward 之前生效）；
//! - 已进入 forward 的请求不能承诺及时取消 → `timeout_unknown`；
//! - 超时返回 ≠ 硬件已停止访问 Buffer → 超时请求的 Buffer 进入**隔离区**，
//!   未获人工确认前不得释放（不得 use-after-free）；
//! - 状态只报 accepted/queued/running/completed/failed/unknown；
//!   **不得把 unknown 报成 failed 或 completed**（05 §5.3）。

use crate::buffer::{Buffer, CompletionToken, DeviceDmaAddress};
use loomos_runtime::ids::OperationId;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum AdmissionError {
    #[error("队列已满（{max}）：拒绝准入，可观测拒绝而非静默排队")]
    QueueFull { max: usize },
    #[error("请求 {0} 已存在（幂等拒绝）")]
    Duplicate(OperationId),
    #[error("deadline {0:?} 小于后端实测最短前向时间，拒绝准入")]
    DeadlineTooTight(Duration),
}

#[derive(Error, Debug, Clone)]
pub enum BackendError {
    #[error("后端返回错误：{0}")]
    Vendor(String),
    #[error("前向超时（deadline {0:?}）：设备状态未知")]
    Timeout(Duration),
    #[error("后端不可用：{0}")]
    Unavailable(String),
}

impl BackendError {
    /// 错误 → 四类故障归因（architecture/13 §13.1）。
    pub fn classify(&self) -> FailureClass {
        match self {
            BackendError::Vendor(_) => FailureClass::ProcessCrash,
            BackendError::Timeout(_) => FailureClass::RequestTimeout,
            BackendError::Unavailable(_) => FailureClass::HardwareInterconnect,
        }
    }
}

/// 故障四类（与 loomos-runtime::supervisor 同义；本 crate 独立定义避免反向依赖循环）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    ProcessCrash,
    RequestTimeout,
    DeviceHang,
    HardwareInterconnect,
}

/// 请求状态（05 §5.3 固定集合）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InferenceState {
    Accepted,
    Queued,
    Running,
    Completed,
    Failed,
    /// 结果无法确认（超时/挂死后）。**不得映射为 Failed**。
    Unknown,
}

impl fmt::Display for InferenceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            InferenceState::Accepted => "accepted",
            InferenceState::Queued => "queued",
            InferenceState::Running => "running",
            InferenceState::Completed => "completed",
            InferenceState::Failed => "failed",
            InferenceState::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// 推理请求。
#[derive(Debug, Clone)]
pub struct InferenceRequest {
    pub id: OperationId,
    pub model: String,
    pub deadline: Duration,
    pub submitted_at: Instant,
}

#[derive(Debug, Clone)]
pub struct SubmitTicket {
    pub request: InferenceRequest,
    pub state: InferenceState,
}

/// 后端输出：携带完成证据，调用方凭此走 Buffer 的 device_complete。
#[derive(Debug, Clone)]
pub struct BackendOutput {
    pub data: Vec<u8>,
    pub completion: CompletionToken,
    pub elapsed: Duration,
}

/// 后端能力声明（05 §5.8 适配表的最小集）。
#[derive(Debug, Clone, Copy)]
pub struct BackendCaps {
    /// 同步阻塞后端为 1；是否真并发需独立取证，不从 demo 推导。
    pub max_concurrent: usize,
    /// 多数厂商 Runtime 无 cancel/abort。
    pub cancel_supported: bool,
}

/// 推理后端契约。**forward 同步阻塞且无 cancel**——按厂商 Runtime 现实建模（05 §5.4）。
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    fn caps(&self) -> BackendCaps;
    fn forward(
        &self,
        req: &InferenceRequest,
        payload: &[u8],
    ) -> Result<BackendOutput, BackendError>;
    /// 实测最短前向时间（准入用；来自 Model Profile 实测，非规格猜测——05 §5.6）。
    fn min_forward_time(&self) -> Duration;
}

/// 模型 Profile（整模型选择依据，05 §5.7；数据来自逐模型实测）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelProfile {
    pub model: String,
    pub backend: String,
    /// 实测：加载时间 ms。
    pub load_ms: u64,
    /// 实测：稳态 RTF。
    pub rtf: f64,
    /// 实测：RSS MB。
    pub rss_mb: f64,
    /// 实测证据等级：direct | doc | estimate（architecture/07 证据等级纪律）。
    pub evidence: String,
}

/// 隔离区条目：持有超时/失败请求的 Buffer（DeviceOwned 态），
/// 未获人工确认前不得释放（12.4 不变量 1 的兜底）。
struct QuarantineEntry {
    operation: OperationId,
    reason: String,
    since: Instant,
    held: crate::buffer::Buffer<crate::buffer::DeviceOwned>,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct BackendStats {
    pub submitted: u64,
    pub queued_now: usize,
    pub completed: u64,
    pub failed: u64,
    pub unknown: u64,
    pub cancelled: u64,
    pub queue_full_rejects: u64,
    pub quarantined_buffers: usize,
    pub total_forward_ms: u128,
}

/// 单 Owner 编排器：普通应用不直接打开多个 Runtime Server（05 §5.2 默认策略）。
pub struct Orchestrator {
    backend: Arc<dyn Backend>,
    queue: VecDeque<InferenceRequest>,
    max_queue: usize,
    running: Option<InferenceRequest>,
    quarantine: Vec<QuarantineEntry>,
    stats: BackendStats,
}

impl Orchestrator {
    pub fn new(backend: Arc<dyn Backend>, max_queue: usize) -> Self {
        Self {
            backend,
            queue: VecDeque::new(),
            max_queue,
            running: None,
            quarantine: Vec::new(),
            stats: BackendStats::default(),
        }
    }

    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    pub fn stats(&self) -> &BackendStats {
        &self.stats
    }

    /// 提交：准入判据明确（队列容量 + deadline 可行性），拒绝可观测。
    pub fn submit(
        &mut self,
        model: &str,
        deadline: Duration,
        payload_len: usize,
    ) -> Result<SubmitTicket, AdmissionError> {
        if self.queue.len() >= self.max_queue {
            self.stats.queue_full_rejects += 1;
            return Err(AdmissionError::QueueFull {
                max: self.max_queue,
            });
        }
        let min = self.backend.min_forward_time();
        if deadline < min {
            return Err(AdmissionError::DeadlineTooTight(min));
        }
        let _ = payload_len;
        let req = InferenceRequest {
            id: OperationId::next(),
            model: model.to_string(),
            deadline,
            submitted_at: Instant::now(),
        };
        self.stats.submitted += 1;
        let ticket = SubmitTicket {
            request: req.clone(),
            state: InferenceState::Accepted,
        };
        self.queue.push_back(req);
        self.stats.queued_now = self.queue.len();
        tracing::info!(op=%ticket.request.id, model, "inference request accepted & queued");
        Ok(ticket)
    }

    /// 取消：只在进入 forward 之前生效（05 §5.4）。
    pub fn cancel_queued(&mut self, op: OperationId) -> Result<InferenceState, AdmissionError> {
        let before = self.queue.len();
        self.queue.retain(|r| r.id != op);
        if self.queue.len() < before {
            self.stats.cancelled += 1;
            self.stats.queued_now = self.queue.len();
            return Ok(InferenceState::Failed);
        }
        if self.running.as_ref().map(|r| r.id) == Some(op) {
            // 已进入同步阻塞调用：不能承诺取消 → Unknown
            return Ok(InferenceState::Unknown);
        }
        Err(AdmissionError::Duplicate(op))
    }

    /// 弹出一个待执行请求（单 Owner 循环）。
    pub fn pop_ready(&mut self) -> Option<SubmitTicket> {
        let req = self.queue.pop_front()?;
        self.stats.queued_now = self.queue.len();
        self.running = Some(req.clone());
        Some(SubmitTicket {
            request: req,
            state: InferenceState::Running,
        })
    }

    /// 执行一次前向（同步阻塞）。payload 与 buffer 二选一：
    /// 带 buffer 时走完整所有权周期；失败时 buffer 进隔离区而非被释放。
    pub fn execute(
        &mut self,
        payload: &[u8],
        dev_addr: DeviceDmaAddress,
    ) -> Result<(BackendOutput, Buffer<crate::buffer::CpuOwned>), BackendError> {
        let req = self.running.clone().expect("execute 之前必须 pop_ready");
        let op = req.id;
        let buf = Buffer::with_data(payload.to_vec());
        let buf = buf.sync_for_device(dev_addr, op).device_take();

        let start = Instant::now();
        let result = self.backend.forward(&req, payload);
        let elapsed = start.elapsed();

        self.running = None;
        match result {
            Ok(mut out) => {
                out.elapsed = elapsed;
                self.stats.completed += 1;
                self.stats.total_forward_ms += elapsed.as_millis();
                match buf.device_complete(out.completion) {
                    Ok(done) => {
                        // 设备写入的结果只在 sync_for_cpu 后对 CPU 可见（12.4 不变量 2）
                        let buf = done.sync_for_cpu(out.data.clone()).cpu_take();
                        Ok((out, buf))
                    }
                    Err((e, back)) => {
                        // 完成证据与请求不符：Buffer 进隔离区（设备状态未知）
                        self.quarantine.push(QuarantineEntry {
                            operation: op,
                            reason: e.to_string(),
                            since: Instant::now(),
                            held: back,
                        });
                        self.stats.quarantined_buffers = self.quarantine.len();
                        Err(BackendError::Vendor("完成证据与请求不匹配".into()))
                    }
                }
            }
            Err(e) => {
                // 12.4 推论：超时返回不代表设备已停止访问 Buffer → 隔离，不释放
                self.quarantine.push(QuarantineEntry {
                    operation: op,
                    reason: e.to_string(),
                    since: Instant::now(),
                    held: buf,
                });
                self.stats.quarantined_buffers = self.quarantine.len();
                match e.classify() {
                    FailureClass::RequestTimeout | FailureClass::DeviceHang => {
                        self.stats.unknown += 1;
                        Err(BackendError::Timeout(elapsed))
                    }
                    FailureClass::ProcessCrash => {
                        self.stats.failed += 1;
                        Err(e)
                    }
                    FailureClass::HardwareInterconnect => {
                        self.stats.unknown += 1;
                        Err(e)
                    }
                }
            }
        }
    }

    /// 隔离区条目（只读视图）。
    pub fn quarantine_entries(&self) -> Vec<(OperationId, String)> {
        self.quarantine
            .iter()
            .map(|q| (q.operation, q.reason.clone()))
            .collect()
    }

    pub fn release_quarantined(&mut self, op: OperationId, confirmed_by: &str) -> bool {
        let before = self.quarantine.len();
        let mut idx = None;
        for (i, q) in self.quarantine.iter().enumerate() {
            if q.operation == op {
                idx = Some(i);
                break;
            }
        }
        if let Some(i) = idx {
            let q = self.quarantine.remove(i);
            let held_for_ms = q.since.elapsed().as_millis() as u64;
            tracing::warn!(op=%op, confirmed_by, held_for_ms, "quarantined buffer approved for release");
            q.held.release_after_human_confirmation();
        }
        self.stats.quarantined_buffers = self.quarantine.len();
        self.quarantine.len() < before
    }

    pub fn model_profile_hint(
        &self,
        profiles: &[ModelProfile],
        model: &str,
    ) -> Option<ModelProfile> {
        profiles.iter().find(|p| p.model == model).cloned()
    }
}

impl Drop for Orchestrator {
    fn drop(&mut self) {
        // 残留隔离区条目：按"未经人工确认"记录后释放（进程退出路径）
        for q in self.quarantine.drain(..) {
            tracing::warn!(
                op=%q.operation,
                reason=%q.reason,
                held_for_ms = q.since.elapsed().as_millis() as u64,
                "orchestrator dropped with quarantined buffer (no human confirmation) — force release, leak evidence recorded"
            );
            q.held.release_after_human_confirmation();
        }
    }
}

// ---------------- Mock 后端（默认档：全自检空机可跑全链，lang/config 的 echo/sine 纪律） ----------------

/// Mock 后端：确定性变换 + 可配置延迟/挂死，用于无真机环境跑通编排契约。
pub struct MockBackend {
    pub delay: Duration,
    pub hang: bool,
    pub min_time: Duration,
    name: String,
}

impl MockBackend {
    pub fn new(name: &str, delay: Duration) -> Self {
        Self {
            delay,
            hang: false,
            min_time: delay,
            name: name.to_string(),
        }
    }

    pub fn hanging(name: &str) -> Self {
        Self {
            delay: Duration::from_millis(50),
            hang: true,
            min_time: Duration::from_millis(50),
            name: name.to_string(),
        }
    }
}

impl Backend for MockBackend {
    fn name(&self) -> &str {
        &self.name
    }

    fn caps(&self) -> BackendCaps {
        BackendCaps {
            max_concurrent: 1,
            cancel_supported: false,
        }
    }

    fn min_forward_time(&self) -> Duration {
        self.min_time
    }

    fn forward(
        &self,
        req: &InferenceRequest,
        payload: &[u8],
    ) -> Result<BackendOutput, BackendError> {
        std::thread::sleep(self.delay);
        if self.hang {
            // 模拟挂死：返回超时错误（真实厂商 runtime 是阻塞不返回；这里可测契约）
            return Err(BackendError::Timeout(req.deadline));
        }
        Ok(BackendOutput {
            // 回声 + 长度标记（echo 档语义）
            data: payload.iter().map(|b| b.wrapping_add(1)).collect(),
            completion: CompletionToken {
                operation: req.id,
                complete: true,
            },
            elapsed: self.delay,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick() -> Orchestrator {
        Orchestrator::new(
            Arc::new(MockBackend::new("mock", Duration::from_millis(1))),
            4,
        )
    }

    #[test]
    fn submit_run_complete_cycle() {
        let mut orch = quick();
        let t = orch.submit("kws", Duration::from_millis(100), 16).unwrap();
        assert_eq!(t.state, InferenceState::Accepted);
        let t2 = orch.pop_ready().unwrap();
        assert_eq!(t2.state, InferenceState::Running);
        let (out, _buf) = orch.execute(&[1, 2, 3], DeviceDmaAddress(0x3000)).unwrap();
        assert_eq!(out.data, vec![2, 3, 4]);
        assert_eq!(orch.stats().completed, 1);
    }

    #[test]
    fn queue_full_rejected_observably() {
        let mut orch = Orchestrator::new(
            Arc::new(MockBackend::new("mock", Duration::from_millis(1))),
            1,
        );
        orch.submit("kws", Duration::from_millis(100), 8).unwrap();
        let err = orch
            .submit("kws", Duration::from_millis(100), 8)
            .unwrap_err();
        assert!(matches!(err, AdmissionError::QueueFull { max: 1 }));
        assert_eq!(orch.stats().queue_full_rejects, 1);
    }

    #[test]
    fn deadline_too_tight_rejected() {
        let mut orch = Orchestrator::new(
            Arc::new(MockBackend::new("mock", Duration::from_millis(50))),
            4,
        );
        let err = orch.submit("asr", Duration::from_millis(1), 8).unwrap_err();
        assert!(matches!(err, AdmissionError::DeadlineTooTight(_)));
    }

    #[test]
    fn cancel_only_before_start() {
        let mut orch = quick();
        let t = orch.submit("asr", Duration::from_millis(100), 8).unwrap();
        assert!(matches!(
            orch.cancel_queued(t.request.id),
            Ok(InferenceState::Failed)
        ));
        // 队列已空，再取消 → Duplicate
        assert!(matches!(
            orch.cancel_queued(t.request.id),
            Err(AdmissionError::Duplicate(_))
        ));
    }

    #[test]
    fn timeout_puts_buffer_in_quarantine_not_released() {
        let mut orch = Orchestrator::new(Arc::new(MockBackend::hanging("vendor-npu")), 4);
        orch.submit("llm", Duration::from_millis(500), 8).unwrap();
        orch.pop_ready().unwrap();
        let err = orch.execute(&[1], DeviceDmaAddress(0x3000)).unwrap_err();
        assert!(matches!(err, BackendError::Timeout(_)));
        // unknown 不算 failed（05 §5.3）
        assert_eq!(orch.stats().unknown, 1);
        assert_eq!(orch.stats().failed, 0);
        // buffer 进隔离区，未人工确认前不得释放
        let q = orch.quarantine_entries();
        assert_eq!(q.len(), 1);
        let op = q[0].0;
        assert!(orch.release_quarantined(op, "operator: power-cycled & verified"));
        assert_eq!(orch.quarantine_entries().len(), 0);
    }

    #[test]
    fn vendor_error_is_failed_not_unknown() {
        struct ErrBackend;
        impl Backend for ErrBackend {
            fn name(&self) -> &str {
                "err"
            }
            fn caps(&self) -> BackendCaps {
                BackendCaps {
                    max_concurrent: 1,
                    cancel_supported: false,
                }
            }
            fn min_forward_time(&self) -> Duration {
                Duration::from_millis(1)
            }
            fn forward(
                &self,
                _r: &InferenceRequest,
                _p: &[u8],
            ) -> Result<BackendOutput, BackendError> {
                Err(BackendError::Vendor("op not supported".into()))
            }
        }
        let mut orch = Orchestrator::new(Arc::new(ErrBackend), 4);
        orch.submit("m", Duration::from_millis(100), 8).unwrap();
        orch.pop_ready().unwrap();
        assert!(orch.execute(&[1], DeviceDmaAddress(0x3000)).is_err());
        assert_eq!(orch.stats().failed, 1);
        assert_eq!(orch.stats().unknown, 0);
    }

    #[test]
    fn state_display_never_confuses_unknown_with_failed() {
        assert_ne!(
            InferenceState::Unknown.to_string(),
            InferenceState::Failed.to_string()
        );
    }
}
