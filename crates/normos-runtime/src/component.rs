//! 组件 Manifest 与生命周期状态机（architecture/03 §3.5）。
//!
//! 状态：Registered → Configured → Ready → Active → Suspended/Failed → Recovering → Terminated。
//! 设备状态与管理设备的服务进程状态分开建模（设备状态在 normos-device）。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

/// 组件 Manifest（第一版 TOML 文件加载，运行时解析为类型化数据；格式本身不构成安全边界）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentManifest {
    pub id: String,
    pub version: u32,
    /// 本组件对外提供的服务接口名。
    #[serde(default)]
    pub provides: Vec<String>,
    /// 依赖的接口名（装配期检查）。
    #[serde(default)]
    pub requires: Vec<String>,
    /// 需要的 capability（装配期授权，逐条授给组件主体）。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// 资源请求（kind 描述字符串，如 "DeviceLease:npu0"）。
    #[serde(default)]
    pub resources: Vec<String>,
    /// 支持的执行模型（same-address-space / isolated-process）。
    #[serde(default)]
    pub execution: Vec<String>,
    /// 健康检查策略描述。
    #[serde(default)]
    pub health: HealthPolicy,
    /// 恢复策略（按故障类别，architecture/13）。
    #[serde(default)]
    pub recovery: RecoverySpec,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HealthPolicy {
    /// "none" | "ping" | "heartbeat"
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub interval_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySpec {
    /// 进程类故障的最大自动重启次数（防重启风暴，architecture/03 §3.6）。
    #[serde(default = "default_max_restarts")]
    pub max_process_restarts: u32,
    /// 设备类故障是否允许自动 reset（默认 false：reset 语义未确认前不作为正式恢复路径）。
    #[serde(default)]
    pub allow_device_reset: bool,
    /// 硬件互连类故障一律人工（断电），此处仅记录提示语。
    #[serde(default)]
    pub hardware_note: String,
}

fn default_max_restarts() -> u32 {
    3
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleState {
    Registered,
    Configured,
    Ready,
    Active,
    Suspended,
    Failed,
    Recovering,
    Terminated,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ComponentError {
    #[error("组件 {0} 未注册")]
    NotRegistered(String),
    #[error("组件 {0} 重复注册")]
    Duplicate(String),
    #[error("非法状态转换 {0:?} → {1:?}（前置条件不满足）")]
    IllegalTransition(LifecycleState, LifecycleState),
    #[error("组件 {0} 依赖的接口 {1} 无人提供")]
    MissingDependency(String, String),
    #[error("组件 {0} 请求的 capability {1} 未获授权")]
    MissingCapability(String, String),
    #[error("组件 {0} 已终止，不可再转换")]
    Terminated(String),
}

/// 合法转换表（architecture/03 §3.5；转换有明确前置条件）。
fn allowed(from: LifecycleState, to: LifecycleState) -> bool {
    use LifecycleState::*;
    matches!(
        (from, to),
        (Registered, Configured)
            | (Configured, Ready)
            | (Ready, Active)
            | (Active, Suspended)
            | (Suspended, Active)
            | (Active, Failed)
            | (Suspended, Failed)
            | (Ready, Failed)
            | (Failed, Recovering)
            | (Active, Recovering)
            | (Recovering, Ready)
            | (Recovering, Failed)
            | (Active, Terminated)
            | (Failed, Terminated)
            | (Suspended, Terminated)
            | (Ready, Terminated)
            | (Registered, Terminated)
            | (Configured, Terminated)
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentRecord {
    pub manifest: ComponentManifest,
    pub state: LifecycleState,
    pub restarts: u32,
}

/// 生命周期管理器：注册、装配检查、状态转换。
pub struct LifecycleManager {
    components: BTreeMap<String, ComponentRecord>,
}

impl LifecycleManager {
    pub fn new() -> Self {
        Self {
            components: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, manifest: ComponentManifest) -> Result<(), ComponentError> {
        if self.components.contains_key(&manifest.id) {
            return Err(ComponentError::Duplicate(manifest.id));
        }
        let id = manifest.id.clone();
        self.components.insert(
            id,
            ComponentRecord {
                manifest,
                state: LifecycleState::Registered,
                restarts: 0,
            },
        );
        Ok(())
    }

    /// 装配检查：依赖是否有人提供；capability 需求是否已授权（授权本体在 CapabilityStore）。
    pub fn check_assembly(
        &self,
        id: &str,
        provided_in_system: &[String],
        granted_capabilities: &[String],
    ) -> Result<(), ComponentError> {
        let rec = self
            .components
            .get(id)
            .ok_or_else(|| ComponentError::NotRegistered(id.to_string()))?;
        for dep in &rec.manifest.requires {
            if !provided_in_system.contains(dep) {
                return Err(ComponentError::MissingDependency(
                    id.to_string(),
                    dep.clone(),
                ));
            }
        }
        for cap in &rec.manifest.capabilities {
            if !granted_capabilities.contains(cap) {
                return Err(ComponentError::MissingCapability(
                    id.to_string(),
                    cap.clone(),
                ));
            }
        }
        Ok(())
    }

    pub fn transition(
        &mut self,
        id: &str,
        to: LifecycleState,
    ) -> Result<LifecycleState, ComponentError> {
        let rec = self
            .components
            .get_mut(id)
            .ok_or_else(|| ComponentError::NotRegistered(id.to_string()))?;
        if rec.state == LifecycleState::Terminated {
            return Err(ComponentError::Terminated(id.to_string()));
        }
        if !allowed(rec.state, to) {
            return Err(ComponentError::IllegalTransition(rec.state, to));
        }
        let prev = rec.state;
        rec.state = to;
        if to == LifecycleState::Active {
            // 从 Failed 恢复到 Active 计数（经 Recovering→Ready→Active 路径）
            if prev == LifecycleState::Ready {
                rec.restarts += 1;
            }
        }
        tracing::info!(component=%id, ?prev, ?to, "component state change");
        Ok(to)
    }

    pub fn get(&self, id: &str) -> Option<&ComponentRecord> {
        self.components.get(id)
    }

    pub fn all(&self) -> impl Iterator<Item = &ComponentRecord> {
        self.components.values()
    }
}

impl Default for LifecycleManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str) -> ComponentManifest {
        ComponentManifest {
            id: id.into(),
            version: 1,
            provides: vec![format!("{id}.v1")],
            requires: vec![],
            capabilities: vec![],
            resources: vec![],
            execution: vec!["same-address-space".into()],
            health: HealthPolicy::default(),
            recovery: RecoverySpec::default(),
        }
    }

    #[test]
    fn happy_path_lifecycle() {
        let mut mgr = LifecycleManager::new();
        mgr.register(manifest("voice")).unwrap();
        mgr.transition("voice", LifecycleState::Configured).unwrap();
        mgr.transition("voice", LifecycleState::Ready).unwrap();
        mgr.transition("voice", LifecycleState::Active).unwrap();
        assert_eq!(mgr.get("voice").unwrap().state, LifecycleState::Active);
    }

    #[test]
    fn illegal_transition_rejected() {
        let mut mgr = LifecycleManager::new();
        mgr.register(manifest("nlu")).unwrap();
        // Registered → Active 跳级非法
        assert!(matches!(
            mgr.transition("nlu", LifecycleState::Active),
            Err(ComponentError::IllegalTransition(_, _))
        ));
    }

    #[test]
    fn assembly_checks_dependencies_and_capabilities() {
        let mut mgr = LifecycleManager::new();
        let mut m = manifest("executor");
        m.requires = vec!["nlu.v1".into()];
        m.capabilities = vec!["device.command".into()];
        mgr.register(m).unwrap();
        assert!(matches!(
            mgr.check_assembly("executor", &[], &[]),
            Err(ComponentError::MissingDependency(_, dep)) if dep == "nlu.v1"
        ));
        assert!(matches!(
            mgr.check_assembly("executor", &["nlu.v1".into()], &[]),
            Err(ComponentError::MissingCapability(_, cap)) if cap == "device.command"
        ));
        assert!(
            mgr.check_assembly("executor", &["nlu.v1".into()], &["device.command".into()])
                .is_ok()
        );
    }

    #[test]
    fn terminated_is_final() {
        let mut mgr = LifecycleManager::new();
        mgr.register(manifest("old")).unwrap();
        mgr.transition("old", LifecycleState::Terminated).unwrap();
        assert!(matches!(
            mgr.transition("old", LifecycleState::Active),
            Err(ComponentError::Terminated(_))
        ));
    }
}
