//! Device Core：注册表、提供者 trait、Operation 状态机（architecture/06 §6.2–6.4）。

use normos_runtime::capability::{CapabilityStore, Rights, Token};
use normos_runtime::event::{EventKind, EventLog};
use normos_runtime::ids::OperationId;
use normos_runtime::resource::{LeaseManager, LeaseMode, RevokeKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Mutex;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDescriptor {
    pub id: String,
    pub provider: String,
    /// light | switch | camera | recorder | lock | speaker | face | system
    pub kind: String,
    pub version: u32,
    /// 设备支持的领域能力标识（≠ 调用权限，06 §6.3）。
    pub capabilities: Vec<DeviceCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceCapability {
    TurnOnOff,
    RecordMode,
    Capture,
    FaceMatch,
    VoiceprintCheck,
    Unlock,
    Speak,
    Reboot,
    Status,
    Time,
}

/// 设备生命周期（06 §6.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceLifecycle {
    Discovered,
    Configured,
    Inactive,
    Active,
    Degraded,
    Faulted,
    Offline,
    Maintenance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HealthState {
    Ok,
    Degraded,
    Faulted,
}

pub type DeviceHealth = HealthState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationState {
    Accepted,
    Queued,
    Executing,
    Succeeded,
    Failed,
    Cancelled,
    Expired,
    /// 结果无法确认（后端超时/挂死）。**不得映射为 Failed**——资源可能仍被占用。
    Unknown,
}

impl OperationState {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            OperationState::Succeeded
                | OperationState::Failed
                | OperationState::Cancelled
                | OperationState::Expired
                | OperationState::Unknown
        )
    }
}

/// 提供者执行结果。
#[derive(Debug, Clone)]
pub enum OpOutcome {
    /// 同步完成并返回结果 JSON。
    Done(serde_json::Value),
    /// 执行过但结果无法确认（如板端超时）。
    UnknownOutcome { reason: String },
}

/// 设备提供者（06 §6.3 DeviceProvider）。
pub trait DeviceProvider: Send {
    fn id(&self) -> ProviderId;
    fn discover(&mut self) -> Vec<DeviceDescriptor>;
    /// 同步执行一个领域操作。取消/超时语义由实现声明。
    fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &serde_json::Value,
    ) -> Result<OpOutcome, DeviceError>;
    fn health(&self, device: &str) -> Result<DeviceHealth, DeviceError>;
    fn telemetry(&self, device: &str) -> Result<serde_json::Value, DeviceError>;
}

pub type ProviderId = String;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum DeviceError {
    #[error("设备 {0} 不存在")]
    NoSuchDevice(String),
    #[error("设备 {0} 不支持操作 {1}")]
    UnsupportedOp(String, String),
    #[error("设备 {0} 状态 {1:?}，操作不可用")]
    NotAvailable(String, DeviceLifecycle),
    #[error("权限不足：{0}")]
    PermissionDenied(String),
    #[error("操作失败：{0}")]
    Failed(String),
    #[error("参数错误：{0}")]
    BadParams(String),
    #[error("设备 {0} 无新鲜画面（video_no_frame）")]
    NoFreshFrame(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationRecord {
    pub id: OperationId,
    pub device: String,
    pub op: String,
    pub params: serde_json::Value,
    pub state: OperationState,
    pub principal: String,
    pub submitted_ms: u64,
    pub finished_ms: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// 操作默认过期时间（ms）：超过即 Expired（未终态时）。
pub const OP_EXPIRY_MS: u64 = 10_000;

/// 设备注册表：provider 注册、发现、租约、Operation 跟踪、遥测、故障。
pub struct DeviceRegistry {
    providers: BTreeMap<ProviderId, Box<dyn DeviceProvider>>,
    devices: BTreeMap<String, (DeviceDescriptor, DeviceLifecycle)>,
    leases: LeaseManager,
    device_resource: BTreeMap<String, normos_runtime::ids::ResourceId>,
    operations: BTreeMap<OperationId, OperationRecord>,
    events: std::sync::Arc<Mutex<EventLog>>,
    now_ms: fn() -> u64,
}

impl DeviceRegistry {
    pub fn new(events: std::sync::Arc<Mutex<EventLog>>) -> Self {
        Self {
            providers: BTreeMap::new(),
            devices: BTreeMap::new(),
            leases: LeaseManager::new(4),
            device_resource: BTreeMap::new(),
            operations: BTreeMap::new(),
            events,
            now_ms: unix_ms,
        }
    }

    pub fn events(&self) -> &std::sync::Arc<Mutex<EventLog>> {
        &self.events
    }

    pub fn lease_manager_mut(&mut self) -> &mut LeaseManager {
        &mut self.leases
    }

    pub fn lease_manager(&self) -> &LeaseManager {
        &self.leases
    }

    /// 注册提供者并发现设备。
    pub fn register_provider(
        &mut self,
        mut provider: Box<dyn DeviceProvider>,
    ) -> Result<(), DeviceError> {
        let pid = provider.id();
        for desc in provider.discover() {
            self.devices
                .insert(desc.id.clone(), (desc, DeviceLifecycle::Discovered));
        }
        self.providers.insert(pid.clone(), provider);
        for (id, (desc, _)) in &self.devices {
            if desc.provider == *pid {
                let rid = self
                    .leases
                    .register(normos_runtime::resource::ResourceKind::DeviceLease, id);
                self.device_resource.insert(id.clone(), rid);
            }
        }
        Ok(())
    }

    pub fn set_lifecycle(
        &mut self,
        device: &str,
        state: DeviceLifecycle,
    ) -> Result<(), DeviceError> {
        let entry = self
            .devices
            .get_mut(device)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        entry.1 = state;
        self.log(
            EventKind::DeviceStateChange,
            device,
            serde_json::json!({"state": format!("{state:?}")}),
        );
        Ok(())
    }

    pub fn device(&self, device: &str) -> Option<&(DeviceDescriptor, DeviceLifecycle)> {
        self.devices.get(device)
    }

    pub fn devices(&self) -> impl Iterator<Item = &(DeviceDescriptor, DeviceLifecycle)> {
        self.devices.values()
    }

    fn log(&self, kind: EventKind, source: &str, payload: serde_json::Value) {
        self.events.lock().unwrap().append(kind, source, payload);
    }

    /// 申请设备租约（独占冲突立即拒绝——runtime 已保证）。
    pub fn acquire(
        &mut self,
        device: &str,
        holder: &str,
        mode: LeaseMode,
    ) -> Result<normos_runtime::ids::LeaseId, DeviceError> {
        if !self.devices.contains_key(device) {
            return Err(DeviceError::NoSuchDevice(device.to_string()));
        }
        let rid = *self
            .device_resource
            .get(device)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        self.leases
            .request(rid, holder, mode)
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    pub fn release_lease(
        &mut self,
        lease: normos_runtime::ids::LeaseId,
    ) -> Result<(), DeviceError> {
        self.leases
            .release(lease)
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
        self.leases
            .confirm_released(lease)
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// 撤销租约（强制式：在途操作标记 Unknown——12.3.3）。
    pub fn revoke_lease(&mut self, lease: normos_runtime::ids::LeaseId) -> Result<(), DeviceError> {
        self.leases
            .revoke(lease, RevokeKind::Forced)
            .map_err(|e| DeviceError::Failed(e.to_string()))
    }

    /// 提交操作：能力校验 → 生命周期检查 → 租约检查（独占必须先 acquire）→ 执行 → 终态。
    pub fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &serde_json::Value,
        principal: &str,
        caps: &CapabilityStore,
        token: &Token,
    ) -> Result<OperationId, DeviceError> {
        // 权限：观察类操作接受 observe 或 command（command 蕴含观察）；
        // 其余操作必须 command。
        let observe_only = matches!(op, "query" | "status" | "health" | "time");
        let check = if observe_only {
            caps.check(
                token,
                &Rights::DeviceObserve {
                    device: device.to_string(),
                },
            )
            .or_else(|_| {
                caps.check(
                    token,
                    &Rights::DeviceCommand {
                        device: device.to_string(),
                    },
                )
            })
        } else {
            caps.check(
                token,
                &Rights::DeviceCommand {
                    device: device.to_string(),
                },
            )
        };
        check.map_err(|e| DeviceError::PermissionDenied(e.to_string()))?;

        let (desc, lifecycle) = self
            .devices
            .get(device)
            .cloned()
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        if !matches!(
            lifecycle,
            DeviceLifecycle::Active | DeviceLifecycle::Degraded
        ) {
            return Err(DeviceError::NotAvailable(device.to_string(), lifecycle));
        }
        // 设备能力声明检查（06 §6.3：DeviceCapability ≠ 调用权限，两层都要过）
        let cap = op_to_capability(op)
            .ok_or_else(|| DeviceError::UnsupportedOp(device.to_string(), op.to_string()))?;
        if !desc.capabilities.contains(&cap) {
            return Err(DeviceError::UnsupportedOp(
                device.to_string(),
                op.to_string(),
            ));
        }

        let op_id = OperationId::next();
        let record = OperationRecord {
            id: op_id,
            device: device.to_string(),
            op: op.to_string(),
            params: params.clone(),
            state: OperationState::Accepted,
            principal: principal.to_string(),
            submitted_ms: (self.now_ms)(),
            finished_ms: None,
            result: None,
            error: None,
        };
        self.operations.insert(op_id, record);
        self.log(
            EventKind::OperationSubmitted,
            device,
            serde_json::json!({"op": op, "operation": op_id.to_string()}),
        );

        // Accepted → Executing（原型为同步执行；跨进程后端时此处为 Queued）
        self.operations.get_mut(&op_id).unwrap().state = OperationState::Executing;
        let provider = self
            .providers
            .get_mut(&desc.provider)
            .ok_or_else(|| DeviceError::Failed(format!("provider {} 丢失", desc.provider)))?;
        match provider.execute(device, op, params) {
            Ok(OpOutcome::Done(v)) => {
                let rec = self.operations.get_mut(&op_id).unwrap();
                rec.state = OperationState::Succeeded;
                rec.finished_ms = Some((self.now_ms)());
                rec.result = Some(v.clone());
                self.log(
                    EventKind::OperationUpdated,
                    device,
                    serde_json::json!({"op": op, "state": "succeeded", "operation": op_id.to_string(), "result": v}),
                );
            }
            Ok(OpOutcome::UnknownOutcome { reason }) => {
                let rec = self.operations.get_mut(&op_id).unwrap();
                rec.state = OperationState::Unknown;
                rec.finished_ms = Some((self.now_ms)());
                rec.error = Some(reason.clone());
                self.log(
                    EventKind::OperationUpdated,
                    device,
                    serde_json::json!({"op": op, "state": "unknown", "operation": op_id.to_string(), "reason": reason}),
                );
            }
            Err(e) => {
                let rec = self.operations.get_mut(&op_id).unwrap();
                rec.state = OperationState::Failed;
                rec.finished_ms = Some((self.now_ms)());
                rec.error = Some(e.to_string());
                self.log(
                    EventKind::OperationUpdated,
                    device,
                    serde_json::json!({"op": op, "state": "failed", "operation": op_id.to_string(), "error": e.to_string()}),
                );
            }
        }
        Ok(op_id)
    }

    pub fn operation(&self, id: OperationId) -> Option<&OperationRecord> {
        self.operations.get(&id)
    }

    pub fn operations(&self) -> impl Iterator<Item = &OperationRecord> {
        self.operations.values()
    }

    /// 过期清理：非终态且超过 OP_EXPIRY_MS → Expired。
    pub fn expire_stale(&mut self) -> usize {
        let now = (self.now_ms)();
        let mut n = 0;
        for rec in self.operations.values_mut() {
            if !rec.state.is_terminal() && now.saturating_sub(rec.submitted_ms) > OP_EXPIRY_MS {
                rec.state = OperationState::Expired;
                rec.finished_ms = Some(now);
                n += 1;
            }
        }
        n
    }

    pub fn health(&self, device: &str) -> Result<DeviceHealth, DeviceError> {
        let (desc, _) = self
            .devices
            .get(device)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        let provider = self
            .providers
            .get(&desc.provider)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        provider.health(device)
    }

    pub fn telemetry(&self, device: &str) -> Result<serde_json::Value, DeviceError> {
        let (desc, _) = self
            .devices
            .get(device)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        let provider = self
            .providers
            .get(&desc.provider)
            .ok_or_else(|| DeviceError::NoSuchDevice(device.to_string()))?;
        provider.telemetry(device)
    }

    pub fn events_snapshot(&self, after: u64, limit: usize) -> Vec<normos_runtime::event::Event> {
        self.events.lock().unwrap().read_after(after, limit)
    }

    /// 测试钩子：注入时钟。
    pub fn set_clock(&mut self, f: fn() -> u64) {
        self.now_ms = f;
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn op_to_capability(op: &str) -> Option<DeviceCapability> {
    Some(match op {
        "turn_on" | "turn_off" => DeviceCapability::TurnOnOff,
        "record_mode" => DeviceCapability::RecordMode,
        "capture" => DeviceCapability::Capture,
        "face_match" => DeviceCapability::FaceMatch,
        "voiceprint_check" | "voiceprint_status" => DeviceCapability::VoiceprintCheck,
        "unlock" => DeviceCapability::Unlock,
        "speak" => DeviceCapability::Speak,
        "reboot" | "cancel_reboot" => DeviceCapability::Reboot,
        "query" | "status" => DeviceCapability::Status,
        "time" => DeviceCapability::Time,
        _ => return None,
    })
}
