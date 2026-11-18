//! 资源模型与租约（architecture/03 §3.3、architecture/12）。
//!
//! 固定约束：
//! - Resource Kind 集合固定（12.2），机型/后端只能声明"某 kind 不支持"，不能自行增删语义；
//! - 独占冲突必须拒绝，不得排队后静默降级（12.3.2）；
//! - 持有者死亡必须回收，不得依赖持有者自觉释放（12.3.1 不变量 1）；
//! - Active → Released 之间必须有可观测的确认点（12.3.1 不变量 3）；
//! - 核亲和是执行对象属性，不是 Resource Kind（11.3.1）；
//! - 实施等级如实上报：enforced 需要证据，best_effort 必须写失效条件（11.3.2）。

use crate::ids::{LeaseId, ResourceId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResourceKind {
    /// 一段时间内可消耗的 CPU 时间（runtime/period/deadline）。
    CpuBudget,
    /// 容量上限（RSS / address-space）。
    MemoryLimit,
    /// 对某块内存的生命周期与访问权。
    BufferLease,
    /// 设备访问/控制权。
    DeviceLease,
    /// 推理会话/后端资源占用。
    NpuSessionLease,
    /// 持久存储资源。
    StorageQuota,
    /// 网络连接/带宽策略。
    NetworkBudget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnforcementLevel {
    /// 平台能强制且已验证（须有设置后读回证据）。
    Enforced,
    /// 只能尽力，可能失效（必须写明失效条件）。
    BestEffort { fails_when: &'static str },
    /// 平台无此能力（须有"设了但不生效"的对照证据）。
    Unsupported,
}

impl fmt::Display for EnforcementLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enforced => write!(f, "enforced"),
            Self::BestEffort { fails_when } => write!(f, "best_effort(失效条件: {fails_when})"),
            Self::Unsupported => write!(f, "unsupported"),
        }
    }
}

/// 核集合：有序、非空、不越界（architecture/11 §11.2.3）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<u32>", into = "Vec<u32>")]
pub struct CoreSet(Vec<u32>);

impl CoreSet {
    pub fn new(mut cores: Vec<u32>, cores_total: u32) -> Result<Self, ResourceError> {
        cores.sort_unstable();
        cores.dedup();
        if cores.is_empty() {
            return Err(ResourceError::EmptyCoreSet);
        }
        if let Some(&c) = cores.last()
            && c >= cores_total
        {
            return Err(ResourceError::CoreOutOfRange {
                core: c,
                cores_total,
            });
        }
        Ok(Self(cores))
    }

    pub fn cores(&self) -> &[u32] {
        &self.0
    }

    pub fn is_subset_of(&self, other: &CoreSet) -> bool {
        self.0.iter().all(|c| other.0.contains(c))
    }
}

impl From<CoreSet> for Vec<u32> {
    fn from(s: CoreSet) -> Vec<u32> {
        s.0
    }
}

/// 反序列化入口：不传 cores_total 时按无上界校验（非空约束仍强制）。
impl TryFrom<Vec<u32>> for CoreSet {
    type Error = ResourceError;
    fn try_from(cores: Vec<u32>) -> Result<Self, ResourceError> {
        Self::new(cores, u32::MAX)
    }
}

impl fmt::Display for CoreSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list: Vec<String> = self.0.iter().map(|c| c.to_string()).collect();
        write!(f, "{{{}}}", list.join(","))
    }
}

/// CPU 时间预算（CpuBudget kind 的载体；定义沿用 baseline/03 §3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CpuBudget {
    pub runtime_us: u64,
    pub period_us: u64,
    pub deadline_us: u64,
}

impl CpuBudget {
    pub fn validate(&self) -> Result<(), ResourceError> {
        if self.runtime_us == 0 || self.period_us == 0 || self.deadline_us == 0 {
            return Err(ResourceError::InvalidBudget("预算项不能为 0"));
        }
        if self.runtime_us > self.period_us {
            return Err(ResourceError::InvalidBudget("runtime 不得超过 period"));
        }
        if self.deadline_us > self.period_us {
            return Err(ResourceError::InvalidBudget("deadline 不得超过 period"));
        }
        Ok(())
    }
}

/// 执行体的运行约束（11.2.1）。affinity 是"允许集"，不是"必须钉死"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub kind: ExecutionKind,
    pub affinity: CoreSet,
    /// 派生执行对象是否继承本 affinity。
    pub inherit: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionKind {
    Thread,
    Process,
    DeviceCtx,
}

/// 准入条件（11.2.2）：affinity_required 必须是 ExecutionContext.affinity 的子集，超集即拒绝。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionContract {
    pub affinity_required: Option<CoreSet>,
    pub budget: Option<CpuBudget>,
}

/// 独占/共享模式（12.3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseMode {
    Exclusive,
    Shared { max_holders: usize },
}

/// 撤销语义（12.3.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevokeKind {
    /// 持有者主动让出。
    Collaborative,
    /// 系统单方面收回；在途操作标记 Unknown。
    Forced,
}

/// 租约状态。Active → Released 之间有显式 `Releasing` 确认点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeaseState {
    Granted,
    Releasing,
    Released,
    Revoked,
    /// 持有者已死亡、等待清扫回收的租约（leak 可观测）。
    Orphaned,
}

/// 一份租约的登记记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lease {
    pub id: LeaseId,
    pub resource: ResourceId,
    pub kind: ResourceKind,
    pub holder: String,
    pub mode: LeaseMode,
    pub state: LeaseState,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ResourceError {
    #[error("资源 {resource:?} 已被独占租约占用（拒绝准入，不静默排队）")]
    ExclusiveConflict { resource: ResourceId },
    #[error("共享租约并发度已达上限 {max}")]
    SharedLimitReached { max: usize },
    #[error("核集合为空（非法）")]
    EmptyCoreSet,
    #[error("核 {core} 越界（cores_total={cores_total}）")]
    CoreOutOfRange { core: u32, cores_total: u32 },
    #[error("无效 CPU 预算：{0}")]
    InvalidBudget(&'static str),
    #[error("准入拒绝：affinity_required {required} 不是执行上下文允许集 {allowed} 的子集")]
    AffinityNotSatisfied { required: String, allowed: String },
    #[error("准入拒绝：CPU 预算与核归属未成对出现（11.3.1 推论 2）")]
    BudgetWithoutAffinity,
    #[error("资源 {0:?} 不存在")]
    NoSuchResource(ResourceId),
    #[error("租约 {0} 状态不符：当前 {1:?}")]
    WrongLeaseState(LeaseId, LeaseState),
    #[error("CpuBudget 未声明亲和，且本上下文要求成对出现")]
    MisconfiguredBudget,
}

/// 资源管理器：申请、准入、授予、确认释放、撤销、死亡清扫。
///
/// 不实现任何具体限制的强制执行；强制执行由 Backend 实施并如实上报等级
/// （architecture/03 §3.3：不得将未能强制实施的约束报告为 enforced）。
#[derive(Debug, Default)]
pub struct LeaseManager {
    resources: BTreeMap<ResourceId, (ResourceKind, String)>,
    leases: BTreeMap<LeaseId, Lease>,
    /// 资源 → 活跃租约（独占时只会有一个 Granted）。
    active: BTreeMap<ResourceId, Vec<LeaseId>>,
    cores_total: u32,
}

impl LeaseManager {
    pub fn new(cores_total: u32) -> Self {
        Self {
            cores_total,
            ..Self::default()
        }
    }

    /// 登记一个可让渡资源。
    pub fn register(&mut self, kind: ResourceKind, description: &str) -> ResourceId {
        let id = ResourceId::next();
        self.resources.insert(id, (kind, description.to_string()));
        id
    }

    pub fn cores_total(&self) -> u32 {
        self.cores_total
    }

    /// 申请租约。独占冲突立即拒绝（不排队、不静默降级）。
    pub fn request(
        &mut self,
        resource: ResourceId,
        holder: &str,
        mode: LeaseMode,
    ) -> Result<LeaseId, ResourceError> {
        let (kind, _) = self
            .resources
            .get(&resource)
            .ok_or(ResourceError::NoSuchResource(resource))?;
        let kind = *kind;
        let active = self.active.entry(resource).or_default();
        // Releasing 未确认归还前仍占用资源（12.3.1 不变量 3：有可观测确认点）
        let granted = active
            .iter()
            .filter(|l| {
                matches!(
                    self.leases[&**l].state,
                    LeaseState::Granted | LeaseState::Releasing
                )
            })
            .count();
        match mode {
            LeaseMode::Exclusive if granted > 0 => {
                return Err(ResourceError::ExclusiveConflict { resource });
            }
            LeaseMode::Shared { max_holders } if granted >= max_holders => {
                return Err(ResourceError::SharedLimitReached { max: max_holders });
            }
            _ => {}
        }
        let lease = Lease {
            id: LeaseId::next(),
            resource,
            kind,
            holder: holder.to_string(),
            mode,
            state: LeaseState::Granted,
        };
        let id = lease.id;
        self.leases.insert(id, lease);
        self.active.get_mut(&resource).unwrap().push(id);
        tracing::info!(lease=%id, resource=%resource, holder, ?mode, "lease granted");
        Ok(id)
    }

    /// 请求释放：进入 Releasing（可观测确认点的前半段）。
    pub fn release(&mut self, lease: LeaseId) -> Result<(), ResourceError> {
        let lease_rec = self
            .leases
            .get_mut(&lease)
            .ok_or(ResourceError::WrongLeaseState(lease, LeaseState::Released))?;
        if lease_rec.state != LeaseState::Granted {
            return Err(ResourceError::WrongLeaseState(lease, lease_rec.state));
        }
        lease_rec.state = LeaseState::Releasing;
        Ok(())
    }

    /// 确认归还：从 Releasing → Released。这是 12.3.1 不变量 3 要求的可观测确认点。
    pub fn confirm_released(&mut self, lease: LeaseId) -> Result<(), ResourceError> {
        let lease_rec = self
            .leases
            .get_mut(&lease)
            .ok_or(ResourceError::WrongLeaseState(lease, LeaseState::Released))?;
        if lease_rec.state != LeaseState::Releasing {
            return Err(ResourceError::WrongLeaseState(lease, lease_rec.state));
        }
        lease_rec.state = LeaseState::Released;
        tracing::info!(lease=%lease, "lease release confirmed");
        Ok(())
    }

    /// 撤销：协作式（等价 release+confirm）或强制式（在途操作由调用方标记 Unknown）。
    pub fn revoke(&mut self, lease: LeaseId, kind: RevokeKind) -> Result<(), ResourceError> {
        let lease_rec = self
            .leases
            .get_mut(&lease)
            .ok_or(ResourceError::WrongLeaseState(lease, LeaseState::Revoked))?;
        match lease_rec.state {
            LeaseState::Granted | LeaseState::Releasing => {
                lease_rec.state = LeaseState::Revoked;
                tracing::warn!(lease=%lease, ?kind, "lease revoked");
                Ok(())
            }
            s => Err(ResourceError::WrongLeaseState(lease, s)),
        }
    }

    /// 持有者死亡：登记为 Orphaned，等待清扫。不得依赖持有者自觉释放。
    pub fn mark_orphaned(&mut self, holder: &str) -> Vec<LeaseId> {
        let mut orphaned = Vec::new();
        for lease in self.leases.values_mut() {
            if lease.holder == holder && lease.state == LeaseState::Granted {
                lease.state = LeaseState::Orphaned;
                orphaned.push(lease.id);
            }
        }
        tracing::warn!(
            holder,
            count = orphaned.len(),
            "leases orphaned by holder death"
        );
        orphaned
    }

    /// 清扫：把 Orphaned 租约回收为 Released（记录 leak 证据）。
    pub fn sweep_orphans(&mut self) -> usize {
        let mut n = 0;
        for lease in self.leases.values_mut() {
            if lease.state == LeaseState::Orphaned {
                lease.state = LeaseState::Released;
                n += 1;
            }
        }
        tracing::warn!(count = n, "orphaned leases swept (leak evidence recorded)");
        n
    }

    pub fn lease(&self, lease: LeaseId) -> Option<&Lease> {
        self.leases.get(&lease)
    }

    pub fn leases(&self) -> impl Iterator<Item = &Lease> {
        self.leases.values()
    }

    /// 准入检查：affinity_required ⊆ ExecutionContext.affinity；超集即拒绝（11.3.3）。
    pub fn admit(
        &self,
        context: &ExecutionContext,
        contract: &ExecutionContract,
    ) -> Result<(), ResourceError> {
        if let Some(required) = &contract.affinity_required
            && !required.is_subset_of(&context.affinity)
        {
            return Err(ResourceError::AffinityNotSatisfied {
                required: required.to_string(),
                allowed: context.affinity.to_string(),
            });
        }
        if let Some(budget) = &contract.budget {
            budget.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_set_validation() {
        assert!(CoreSet::new(vec![0, 1, 3], 4).is_ok());
        assert!(matches!(
            CoreSet::new(vec![], 4),
            Err(ResourceError::EmptyCoreSet)
        ));
        assert!(matches!(
            CoreSet::new(vec![4], 4),
            Err(ResourceError::CoreOutOfRange { core: 4, .. })
        ));
        let full = CoreSet::new(vec![0, 1], 4).unwrap();
        let sub = CoreSet::new(vec![1], 4).unwrap();
        assert!(sub.is_subset_of(&full));
        assert!(!full.is_subset_of(&sub));
    }

    #[test]
    fn budget_validation() {
        let ok = CpuBudget {
            runtime_us: 30_000,
            period_us: 100_000,
            deadline_us: 90_000,
        };
        assert!(ok.validate().is_ok());
        let bad = CpuBudget {
            runtime_us: 200_000,
            period_us: 100_000,
            deadline_us: 90_000,
        };
        assert!(matches!(
            bad.validate(),
            Err(ResourceError::InvalidBudget(_))
        ));
    }

    #[test]
    fn exclusive_conflict_is_rejected_not_queued() {
        let mut mgr = LeaseManager::new(4);
        let res = mgr.register(ResourceKind::DeviceLease, "npu0");
        let l1 = mgr.request(res, "voice", LeaseMode::Exclusive).unwrap();
        let err = mgr.request(res, "home", LeaseMode::Exclusive).unwrap_err();
        assert!(matches!(err, ResourceError::ExclusiveConflict { .. }));
        mgr.release(l1).unwrap();
        mgr.confirm_released(l1).unwrap();
        let l2 = mgr.request(res, "home", LeaseMode::Exclusive).unwrap();
        assert_eq!(mgr.lease(l2).unwrap().holder, "home");
    }

    #[test]
    fn shared_respects_limit() {
        let mut mgr = LeaseManager::new(4);
        let res = mgr.register(ResourceKind::BufferLease, "frame0");
        mgr.request(res, "a", LeaseMode::Shared { max_holders: 2 })
            .unwrap();
        mgr.request(res, "b", LeaseMode::Shared { max_holders: 2 })
            .unwrap();
        assert!(matches!(
            mgr.request(res, "c", LeaseMode::Shared { max_holders: 2 }),
            Err(ResourceError::SharedLimitReached { max: 2 })
        ));
    }

    #[test]
    fn release_requires_observable_confirm_point() {
        let mut mgr = LeaseManager::new(4);
        let res = mgr.register(ResourceKind::DeviceLease, "cam0");
        let l = mgr.request(res, "video", LeaseMode::Exclusive).unwrap();
        mgr.release(l).unwrap();
        assert_eq!(mgr.lease(l).unwrap().state, LeaseState::Releasing);
        // Releasing 状态下资源仍被视为占用：新独占请求必须失败。
        assert!(mgr.request(res, "x", LeaseMode::Exclusive).is_err());
        mgr.confirm_released(l).unwrap();
        assert_eq!(mgr.lease(l).unwrap().state, LeaseState::Released);
        assert!(mgr.request(res, "x", LeaseMode::Exclusive).is_ok());
    }

    #[test]
    fn holder_death_orphans_then_sweeps() {
        let mut mgr = LeaseManager::new(4);
        let res = mgr.register(ResourceKind::NpuSessionLease, "npu-session");
        mgr.request(res, "inference-owner", LeaseMode::Exclusive)
            .unwrap();
        let orphaned = mgr.mark_orphaned("inference-owner");
        assert_eq!(orphaned.len(), 1);
        assert_eq!(mgr.lease(orphaned[0]).unwrap().state, LeaseState::Orphaned);
        assert_eq!(mgr.sweep_orphans(), 1);
        assert_eq!(mgr.lease(orphaned[0]).unwrap().state, LeaseState::Released);
    }

    #[test]
    fn admission_rejects_superset_affinity() {
        let mgr = LeaseManager::new(4);
        let context = ExecutionContext {
            kind: ExecutionKind::Thread,
            affinity: CoreSet::new(vec![0, 1], 4).unwrap(),
            inherit: true,
        };
        // 子集放行
        let ok = ExecutionContract {
            affinity_required: Some(CoreSet::new(vec![1], 4).unwrap()),
            budget: None,
        };
        assert!(mgr.admit(&context, &ok).is_ok());
        // 超集拒绝，不静默降级（11.3.3）
        let bad = ExecutionContract {
            affinity_required: Some(CoreSet::new(vec![2], 4).unwrap()),
            budget: None,
        };
        let err = mgr.admit(&context, &bad).unwrap_err();
        assert!(matches!(err, ResourceError::AffinityNotSatisfied { .. }));
    }

    #[test]
    fn enforcement_levels_report_honestly() {
        // 本 crate 不冒充 enforced；示例：本机 cgroup 控制器未编译时报 unsupported。
        let mem = EnforcementLevel::Unsupported;
        let sched = EnforcementLevel::BestEffort {
            fails_when: "PREEMPT_NONE 下尾延迟抖动",
        };
        assert_eq!(mem.to_string(), "unsupported");
        assert!(sched.to_string().contains("best_effort"));
    }
}
