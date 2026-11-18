//! Capability：权限对象、子集委托、generation 撤销、审计（architecture/03 §3.4）。
//!
//! 纪律：
//! - 类型参数防误用，但真正权限由服务端校验——本模块就是服务端校验点；
//! - 动态撤销靠 generation，不依赖 Rust 生命周期；
//! - 不把全局 Admin 发给普通业务组件。

use crate::resource::ResourceKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

static NEXT: AtomicU64 = AtomicU64::new(1);

/// capability 票据 ID（透明序列化为 u64）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityId(u64);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rights {
    /// 读某 namespace。
    ConfigRead { namespace: String },
    /// 写某 namespace（子 namespace 权限不得自动扩大父 namespace 授予范围——见 architecture/04 §4.3）。
    ConfigWrite { namespace: String },
    /// 观察某设备。
    DeviceObserve { device: String },
    /// 向某设备下达命令。
    DeviceCommand { device: String },
    /// 申请某类资源。
    ResourceRequest { kind: ResourceKind },
    /// 提交推理请求。
    NpuSubmit,
    /// 全局管理权限（只授予 supervisor / 装配层，绝不授予普通业务组件）。
    Admin,
}

impl Rights {
    pub fn kind(&self) -> &'static str {
        match self {
            Rights::ConfigRead { .. } => "config.read",
            Rights::ConfigWrite { .. } => "config.write",
            Rights::DeviceObserve { .. } => "device.observe",
            Rights::DeviceCommand { .. } => "device.command",
            Rights::ResourceRequest { .. } => "resource.request",
            Rights::NpuSubmit => "npu.submit",
            Rights::Admin => "admin",
        }
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Rights::ConfigRead { namespace } => write!(f, "config.read({namespace})"),
            Rights::ConfigWrite { namespace } => write!(f, "config.write({namespace})"),
            Rights::DeviceObserve { device } => write!(f, "device.observe({device})"),
            Rights::DeviceCommand { device } => write!(f, "device.command({device})"),
            Rights::ResourceRequest { kind } => write!(f, "resource.request({kind:?})"),
            Rights::NpuSubmit => write!(f, "npu.submit"),
            Rights::Admin => write!(f, "admin"),
        }
    }
}

/// 一张 capability 票据。generation 被撤销链路作废。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Token {
    pub id: CapabilityId,
    pub subject: String,
    pub right: Rights,
    pub generation: u64,
}

impl CapabilityId {
    pub fn next() -> Self {
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl fmt::Display for CapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cap-{}", self.0)
    }
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum CapabilityError {
    #[error("拒绝：{subject} 不持有 {right}")]
    NotGranted { subject: String, right: String },
    #[error("拒绝：capability {0} 已被撤销（generation 失效）")]
    Revoked(CapabilityId),
    #[error("拒绝委托：{0} 的权限必须是父票据的子集，不得扩大")]
    EscalationDenied(CapabilityId),
    #[error("未知 capability {0}")]
    Unknown(CapabilityId),
    #[error("Admin 权限不得授予普通业务组件 {0}")]
    AdminNotGrantable(String),
}

#[derive(Debug, Default)]
pub struct CapabilityStore {
    tokens: BTreeMap<u64, Token>,
    revoked: BTreeMap<u64, ()>,
    audit: Vec<String>,
}

impl CapabilityStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 授权。Admin 只允许授予 supervisor/装配层（subject 以 "supervisor"/"installer" 开头）。
    pub fn grant(&mut self, subject: &str, right: Rights) -> Result<Token, CapabilityError> {
        if matches!(right, Rights::Admin)
            && !(subject.starts_with("supervisor") || subject.starts_with("installer"))
        {
            return Err(CapabilityError::AdminNotGrantable(subject.to_string()));
        }
        let token = Token {
            id: CapabilityId::next(),
            subject: subject.to_string(),
            right,
            generation: 1,
        };
        self.audit
            .push(format!("grant {} -> {}", subject, token.right));
        self.tokens.insert(token.id.0, token.clone());
        Ok(token)
    }

    /// 委托：子票据的权限必须是父票据的子集（不得扩大）。
    pub fn delegate(
        &mut self,
        parent: &Token,
        child_subject: &str,
        right: Rights,
    ) -> Result<Token, CapabilityError> {
        let parent_rec = self
            .tokens
            .get(&parent.id.0)
            .ok_or(CapabilityError::Unknown(parent.id))?;
        if parent_rec.generation != parent.generation {
            return Err(CapabilityError::Revoked(parent.id));
        }
        if !right_is_subset(&right, &parent_rec.right) {
            return Err(CapabilityError::EscalationDenied(parent.id));
        }
        let token = Token {
            id: CapabilityId::next(),
            subject: child_subject.to_string(),
            right,
            generation: parent.generation,
        };
        self.audit.push(format!(
            "delegate {} {} -> {}",
            parent.id, token.right, child_subject
        ));
        self.tokens.insert(token.id.0, token.clone());
        Ok(token)
    }

    /// 撤销：generation 作废，所有持有旧代票据的调用全部失败。
    pub fn revoke(&mut self, token: &Token) -> Result<(), CapabilityError> {
        let rec = self
            .tokens
            .get_mut(&token.id.0)
            .ok_or(CapabilityError::Unknown(token.id))?;
        rec.generation += 1;
        self.revoked.insert(token.id.0, ());
        self.audit
            .push(format!("revoke {} ({})", token.id, token.right));
        tracing::warn!(cap=%token.id, right=%token.right, "capability revoked");
        Ok(())
    }

    /// 校验一张票据仍然有效（generation 未失效）。
    pub fn validate(&self, token: &Token) -> Result<&Token, CapabilityError> {
        let rec = self
            .tokens
            .get(&token.id.0)
            .ok_or(CapabilityError::Unknown(token.id))?;
        if rec.generation != token.generation {
            return Err(CapabilityError::Revoked(token.id));
        }
        Ok(rec)
    }

    /// 检查某主体是否持有某项权利：票据必须有效（generation 未失效），
    /// 权限按主体聚合（主体名下任一未撤销票据覆盖 required 即通过）。
    /// 一次请求跨多设备操作时，主体只需出示一张自己的有效票据。
    pub fn check(&self, token: &Token, required: &Rights) -> Result<(), CapabilityError> {
        let rec = self.validate(token)?;
        if self.holds(&rec.subject, required) {
            Ok(())
        } else {
            Err(CapabilityError::NotGranted {
                subject: rec.subject.clone(),
                right: required.to_string(),
            })
        }
    }

    /// 按主体直接查询（用于装配期检查，非热路径）。
    pub fn holds(&self, subject: &str, required: &Rights) -> bool {
        self.tokens
            .values()
            .filter(|t| t.subject == subject && !self.revoked.contains_key(&t.id.0))
            .any(|t| right_is_subset(required, &t.right))
    }

    pub fn audit(&self) -> &[String] {
        &self.audit
    }
}

/// 权利子集判定：`required` 是否被 `held` 覆盖。
/// namespace 前缀匹配只允许"精确或更深的子路径"，且 ConfigWrite 不被 ConfigRead 覆盖。
fn right_is_subset(required: &Rights, held: &Rights) -> bool {
    if matches!(held, Rights::Admin) {
        return !matches!(required, Rights::Admin) || matches!(required, Rights::Admin);
    }
    match (required, held) {
        (Rights::ConfigRead { namespace: want }, Rights::ConfigRead { namespace: got })
        | (Rights::ConfigWrite { namespace: want }, Rights::ConfigWrite { namespace: got }) => {
            ns_prefix_match(want, got)
        }
        (Rights::DeviceObserve { device: want }, Rights::DeviceObserve { device: got })
        | (Rights::DeviceCommand { device: want }, Rights::DeviceCommand { device: got }) => {
            want == got
        }
        (Rights::ResourceRequest { kind: want }, Rights::ResourceRequest { kind: got }) => {
            want == got
        }
        (Rights::NpuSubmit, Rights::NpuSubmit) => true,
        _ => false,
    }
}

/// 授予范围只覆盖自身及其更深的子路径；
/// 子 namespace 的授权不会自动扩大到父 namespace（architecture/04 §4.3）。
fn ns_prefix_match(want: &str, got: &str) -> bool {
    want == got || want.starts_with(&format!("{got}/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NamespacePath;

    #[test]
    fn grant_check_revoke() {
        let mut store = CapabilityStore::new();
        let tok = store
            .grant(
                "voice",
                Rights::ConfigWrite {
                    namespace: "services/voice".into(),
                },
            )
            .unwrap();
        let need = Rights::ConfigWrite {
            namespace: "services/voice/asr".into(),
        };
        assert!(store.check(&tok, &need).is_ok());
        store.revoke(&tok).unwrap();
        // 旧代票据全部失效
        assert!(matches!(
            store.check(&tok, &need),
            Err(CapabilityError::Revoked(_))
        ));
    }

    #[test]
    fn delegation_cannot_escalate() {
        let mut store = CapabilityStore::new();
        let parent = store
            .grant(
                "installer",
                Rights::DeviceCommand {
                    device: "light.keting".into(),
                },
            )
            .unwrap();
        // 子集委托 OK
        let child = store
            .delegate(
                &parent,
                "executor",
                Rights::DeviceCommand {
                    device: "light.keting".into(),
                },
            )
            .unwrap();
        assert!(
            store
                .check(
                    &child,
                    &Rights::DeviceCommand {
                        device: "light.keting".into()
                    }
                )
                .is_ok()
        );
        // 扩大被拒
        let bad = store
            .delegate(
                &parent,
                "executor",
                Rights::DeviceCommand {
                    device: "door.lock".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(bad, CapabilityError::EscalationDenied(_)));
    }

    #[test]
    fn admin_only_for_supervisor() {
        let mut store = CapabilityStore::new();
        assert!(matches!(
            store.grant("voice", Rights::Admin),
            Err(CapabilityError::AdminNotGrantable(_))
        ));
        assert!(store.grant("supervisor", Rights::Admin).is_ok());
    }

    #[test]
    fn namespace_scope_does_not_expand_upward() {
        let mut store = CapabilityStore::new();
        // 只授予 asr 子树，不能读 services/voice 父级
        let tok = store
            .grant(
                "asr",
                Rights::ConfigRead {
                    namespace: "services/voice/asr".into(),
                },
            )
            .unwrap();
        assert!(
            store
                .check(
                    &tok,
                    &Rights::ConfigRead {
                        namespace: "services/voice".into()
                    }
                )
                .is_err()
        );
        assert!(
            store
                .check(
                    &tok,
                    &Rights::ConfigRead {
                        namespace: "services/voice/asr/profiles".into()
                    }
                )
                .is_ok()
        );
    }

    #[test]
    fn config_read_does_not_cover_write() {
        let mut store = CapabilityStore::new();
        let tok = store
            .grant(
                "monitor",
                Rights::ConfigRead {
                    namespace: "system".into(),
                },
            )
            .unwrap();
        assert!(
            store
                .check(
                    &tok,
                    &Rights::ConfigWrite {
                        namespace: "system/logging".into()
                    }
                )
                .is_err()
        );
    }

    #[test]
    fn namespace_path_validation_rejects_escape() {
        assert!(NamespacePath::new("services/voice/asr").is_ok());
        assert!(NamespacePath::new("services//voice").is_err()); // 空段
        assert!(NamespacePath::new("services/../system").is_err()); // 路径逃逸
        assert!(NamespacePath::new("/services").is_err()); // 前导分隔
        assert!(NamespacePath::new("services/voice/").is_err()); // 尾部分隔
    }
}
