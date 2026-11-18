//! Supervisor / Recovery（architecture/03 §3.6、architecture/13）。
//!
//! 三条禁令（architecture/13 §13.2）：
//! 1. 禁止统一 `catch Exception → restart()`——不同故障类别恢复动作不同；
//! 2. 超时 ≠ 取消成功——超时返回不说明设备已停止访问 Buffer；
//! 3. `reset()` API 存在 ≠ 语义已确认——语义未确认前不得作为正式恢复路径。
//!
//! 四类故障（13.1）每一次都必须能归因；恢复等级 L1/L2/L3 逐级声明。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};
use thiserror::Error;

/// 四类故障（分类固定，判据由机型补——13.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    /// 进程退出/未捕获异常/自身崩溃。恢复上限：重启进程 + 回收其 Lease。
    ProcessCrash,
    /// 调用返回超时，但设备状态未知。不得假定设备已停止。
    RequestTimeout,
    /// 设备不再响应，进程仍活。恢复上限由机型声明（可否 reset / 是否需断电）。
    DeviceHang,
    /// 总线挂死、无 printk、整机异常。恢复上限：断电（人工/看门狗）。
    HardwareInterconnect,
}

impl fmt::Display for FailureClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FailureClass::ProcessCrash => write!(f, "process_crash"),
            FailureClass::RequestTimeout => write!(f, "request_timeout"),
            FailureClass::DeviceHang => write!(f, "device_hang"),
            FailureClass::HardwareInterconnect => write!(f, "hardware_interconnect"),
        }
    }
}

/// 恢复等级（13.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryLevel {
    /// L1 进程重启：退出并重启该服务，回收其资源。
    L1ProcessRestart,
    /// L2 设备 reset：调用厂商 reset 接口（语义未确认前不得作为正式路径）。
    L2DeviceReset,
    /// L3 整机断电：只能人工/看门狗。软件路径到此为止。
    L3PowerCycleManual,
}

/// 已执行的恢复动作记录（可追溯，13.5）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryAction {
    pub component: String,
    pub failure: FailureClass,
    pub level: RecoveryLevel,
    pub outcome: RecoveryOutcome,
    pub note: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryOutcome {
    /// 恢复动作已执行且确认有效（如重启后同一请求可成功）。
    Recovered,
    /// 动作已执行，效果未确认。
    Unconfirmed,
    /// 升级到人工处理（记录，不自行动危险操作）。
    EscalatedToHuman,
    /// 按策略不自动恢复（如重启风暴熔断）。
    PolicyDenied,
}

#[derive(Error, Debug, Clone)]
pub enum SupervisorError {
    #[error("组件 {0} 未登记到 supervisor")]
    UnknownComponent(String),
    #[error("组件 {0} 已进入熔断（重启风暴保护），等待人工处理")]
    RestartStormBreaker(String),
}

/// 每类故障的恢复策略（禁令 1：不同类别动作不同）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryPolicy {
    /// 进程类：自动重启上限 + 退避。
    pub max_process_restarts: u32,
    pub restart_backoff_ms: u64,
    /// 熔断窗口内的重启次数上限（防风暴）。
    pub breaker_window_restarts: u32,
    pub breaker_window: Duration,
    /// 设备类：是否允许 L2（reset）。默认 false（禁令 3）。
    pub allow_device_reset: bool,
}

impl Default for RecoveryPolicy {
    fn default() -> Self {
        Self {
            max_process_restarts: 3,
            restart_backoff_ms: 200,
            breaker_window_restarts: 5,
            breaker_window: Duration::from_secs(60),
            allow_device_reset: false,
        }
    }
}

#[derive(Debug, Clone)]
struct ComponentHealth {
    restarts_total: u32,
    recent_restarts: VecDeque<Instant>,
    breaker_open: bool,
}

/// Supervisor：登记组件、按类别实施恢复、记录动作。
/// 它只决定"做什么等级的恢复"；实际回收（Lease 等）由调用方按返回的动作执行。
pub struct Supervisor {
    policy: RecoveryPolicy,
    health: std::collections::BTreeMap<String, ComponentHealth>,
    pub history: Vec<RecoveryAction>,
}

impl Supervisor {
    pub fn new(policy: RecoveryPolicy) -> Self {
        Self {
            policy,
            health: Default::default(),
            history: Vec::new(),
        }
    }

    pub fn register(&mut self, component: &str) {
        self.health.insert(
            component.to_string(),
            ComponentHealth {
                restarts_total: 0,
                recent_restarts: VecDeque::new(),
                breaker_open: false,
            },
        );
    }

    /// 故障上报 → 归类 → 产出恢复动作。**每次故障必须可归因**（13.1 硬约束）。
    pub fn report_failure(
        &mut self,
        component: &str,
        failure: FailureClass,
        evidence: &str,
    ) -> Result<RecoveryAction, SupervisorError> {
        tracing::warn!(component, %failure, evidence, "failure classified");
        if !self.health.contains_key(component) {
            return Err(SupervisorError::UnknownComponent(component.to_string()));
        }
        let action = match failure {
            FailureClass::ProcessCrash => self.handle_process_crash(component),
            FailureClass::RequestTimeout => RecoveryAction {
                component: component.to_string(),
                failure,
                level: RecoveryLevel::L1ProcessRestart,
                outcome: RecoveryOutcome::PolicyDenied,
                note: "请求超时：先按 timeout_unknown 处理在途操作，不直接重启；在途 Buffer 不得释放（13.4）".into(),
            },
            FailureClass::DeviceHang => self.handle_device_hang(component),
            FailureClass::HardwareInterconnect => RecoveryAction {
                component: component.to_string(),
                failure,
                level: RecoveryLevel::L3PowerCycleManual,
                outcome: RecoveryOutcome::EscalatedToHuman,
                note: "硬件/互连故障：软件路径无恢复能力，升级人工断电（13.3 L3）".into(),
            },
        };
        self.history.push(action.clone());
        Ok(action)
    }

    fn handle_process_crash(&mut self, component: &str) -> RecoveryAction {
        let h = self.health.get_mut(component).unwrap();
        if h.breaker_open {
            return RecoveryAction {
                component: component.to_string(),
                failure: FailureClass::ProcessCrash,
                level: RecoveryLevel::L1ProcessRestart,
                outcome: RecoveryOutcome::PolicyDenied,
                note: "熔断已打开：等待人工处理，不自动重启（防重启风暴）".into(),
            };
        }
        let now = Instant::now();
        h.recent_restarts.push_back(now);
        while h.recent_restarts.len() > self.policy.breaker_window_restarts as usize {
            h.recent_restarts.pop_front();
        }
        let in_window = h.recent_restarts.len();
        if h.restarts_total >= self.policy.max_process_restarts
            && in_window >= self.policy.breaker_window_restarts as usize
        {
            h.breaker_open = true;
            return RecoveryAction {
                component: component.to_string(),
                failure: FailureClass::ProcessCrash,
                level: RecoveryLevel::L1ProcessRestart,
                outcome: RecoveryOutcome::EscalatedToHuman,
                note: format!("窗口内重启 {in_window} 次触发熔断：停止自动重启，升级人工"),
            };
        }
        h.restarts_total += 1;
        let count = h.restarts_total;
        RecoveryAction {
            component: component.to_string(),
            failure: FailureClass::ProcessCrash,
            level: RecoveryLevel::L1ProcessRestart,
            outcome: RecoveryOutcome::Unconfirmed,
            note: format!(
                "L1 进程重启（第 {count} 次，上限 {}），退避 {} ms；持有 Lease 按 Orphaned 清扫",
                self.policy.max_process_restarts, self.policy.restart_backoff_ms
            ),
        }
    }

    fn handle_device_hang(&mut self, component: &str) -> RecoveryAction {
        if !self.policy.allow_device_reset {
            return RecoveryAction {
                component: component.to_string(),
                failure: FailureClass::DeviceHang,
                level: RecoveryLevel::L3PowerCycleManual,
                outcome: RecoveryOutcome::EscalatedToHuman,
                note: "设备挂死且 reset 语义未确认（禁令 3）：不调用 reset，升级人工断电".into(),
            };
        }
        RecoveryAction {
            component: component.to_string(),
            failure: FailureClass::DeviceHang,
            level: RecoveryLevel::L2DeviceReset,
            outcome: RecoveryOutcome::Unconfirmed,
            note: "L2 设备 reset 已授权且语义已确认；在途操作标记 Unknown".into(),
        }
    }

    /// 恢复动作执行后的确认回执（"恢复后 Lease 状态可查"——13.5）。
    pub fn confirm_recovery(&mut self, component: &str, recovered: bool) -> Option<RecoveryAction> {
        let last = self
            .history
            .iter_mut()
            .rev()
            .find(|a| a.component == component)?;
        last.outcome = if recovered {
            RecoveryOutcome::Recovered
        } else {
            RecoveryOutcome::Unconfirmed
        };
        Some(last.clone())
    }

    pub fn is_breaker_open(&self, component: &str) -> bool {
        self.health
            .get(component)
            .map(|h| h.breaker_open)
            .unwrap_or(false)
    }

    pub fn restarts_total(&self, component: &str) -> u32 {
        self.health
            .get(component)
            .map(|h| h.restarts_total)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn process_crash_restarts_with_backoff_then_breaker() {
        let mut sup = Supervisor::new(RecoveryPolicy {
            max_process_restarts: 2,
            breaker_window_restarts: 3,
            ..Default::default()
        });
        sup.register("voice");
        for _ in 0..2 {
            let a = sup
                .report_failure("voice", FailureClass::ProcessCrash, "exit code 101")
                .unwrap();
            assert_eq!(a.level, RecoveryLevel::L1ProcessRestart);
        }
        // 第 3 次达到 max_restarts 且窗口满 → 熔断
        let a = sup
            .report_failure("voice", FailureClass::ProcessCrash, "exit code 101")
            .unwrap();
        assert_eq!(a.outcome, RecoveryOutcome::EscalatedToHuman);
        assert!(sup.is_breaker_open("voice"));
        // 熔断后不再自动重启
        let a = sup
            .report_failure("voice", FailureClass::ProcessCrash, "again")
            .unwrap();
        assert_eq!(a.outcome, RecoveryOutcome::PolicyDenied);
    }

    #[test]
    fn request_timeout_never_assumes_device_stopped() {
        let mut sup = Supervisor::new(RecoveryPolicy::default());
        sup.register("inference");
        let a = sup
            .report_failure(
                "inference",
                FailureClass::RequestTimeout,
                "forward deadline exceeded",
            )
            .unwrap();
        // 超时不重启设备：按 timeout_unknown 处理
        assert_eq!(a.outcome, RecoveryOutcome::PolicyDenied);
        assert!(a.note.contains("timeout_unknown"));
        assert!(a.note.contains("不得释放"));
    }

    #[test]
    fn device_hang_escalates_when_reset_unconfirmed() {
        // 默认策略：reset 语义未确认 → 禁令 3 生效，升级人工
        let mut sup = Supervisor::new(RecoveryPolicy::default());
        sup.register("npu");
        let a = sup
            .report_failure("npu", FailureClass::DeviceHang, "consecutive timeouts")
            .unwrap();
        assert_eq!(a.level, RecoveryLevel::L3PowerCycleManual);
        assert_eq!(a.outcome, RecoveryOutcome::EscalatedToHuman);
    }

    #[test]
    fn hardware_interconnect_is_always_manual() {
        let mut sup = Supervisor::new(RecoveryPolicy::default());
        sup.register("axi");
        let a = sup
            .report_failure(
                "axi",
                FailureClass::HardwareInterconnect,
                "no printk, bus dead",
            )
            .unwrap();
        assert_eq!(a.level, RecoveryLevel::L3PowerCycleManual);
        assert_eq!(a.outcome, RecoveryOutcome::EscalatedToHuman);
    }

    #[test]
    fn unknown_component_rejected() {
        let mut sup = Supervisor::new(RecoveryPolicy::default());
        assert!(
            sup.report_failure("ghost", FailureClass::ProcessCrash, "e")
                .is_err()
        );
    }

    #[test]
    fn recovery_confirmation_updates_outcome() {
        let mut sup = Supervisor::new(RecoveryPolicy::default());
        sup.register("voice");
        sup.report_failure("voice", FailureClass::ProcessCrash, "panic")
            .unwrap();
        let a = sup.confirm_recovery("voice", true).unwrap();
        assert_eq!(a.outcome, RecoveryOutcome::Recovered);
    }
}
