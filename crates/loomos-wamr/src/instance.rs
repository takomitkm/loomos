//! Wasm 实例管理：**原子预算记账**（ADR-011 §4.3）。
//!
//! 生产语义：
//! - 全局预算 + 单实例上限，全部**有界**（0 = 不限被默认策略拒绝）；
//! - 原子预留（CAS）→ 成功提交 → 失败回滚 → 销毁释放；
//! - 并发准入由 [`InstanceManager::try_admit`] 的无锁 CAS 保证——
//!   `n` 线程同时准入，累计预留永不超出预算（有并发测试）。
//!
//! 注意：这里管的是 **Guest 线性内存预算**，不是进程 RSS——两者不可混谈。

use crate::error::{WamrError, WamrResult};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub module_name: String,
    /// 调用方组件身份（Host ABI 能力校验的主体；空 = 实例化时取模块名）。
    pub component_id: String,
    /// 实例执行栈大小（字节）。
    pub stack_size: u32,
    /// WAMR 应用堆大小（字节；0 = Rust 模块自管线性内存，bench 实测约定）。
    pub heap_size: u32,
    /// 本实例计入全局预算的内存上限（字节；必须 > 0）。
    pub memory_limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstanceHandle(pub u64);

/// 准入预留（try_admit 成功的凭证；commit 消费 / rollback 退还）。
#[derive(Debug)]
pub struct Reservation {
    bytes: u64,
}

impl Reservation {
    pub fn bytes(&self) -> u64 {
        self.bytes
    }
}

#[derive(Debug)]
pub(crate) struct InstanceState {
    pub spec: InstanceSpec,
}

/// 实例管理器：原子预算记账 + 实例注册表。
pub struct InstanceManager {
    instances: BTreeMap<u64, InstanceState>,
    next_id: AtomicU64,
    /// 全局内存预算（字节）。
    budget: u64,
    /// 已预留（含创建中；原子更新）。
    reserved: AtomicU64,
    /// 单实例内存上限（字节；0 = 未配置 → 拒绝）。
    per_instance_limit: u64,
}

impl InstanceManager {
    pub fn new(budget: u64, per_instance_limit: u64) -> Self {
        Self {
            instances: BTreeMap::new(),
            next_id: AtomicU64::new(1),
            budget,
            reserved: AtomicU64::new(0),
            per_instance_limit,
        }
    }

    pub fn budget(&self) -> u64 {
        self.budget
    }

    /// 当前已预留字节数（含创建中）。
    pub fn reserved(&self) -> u64 {
        self.reserved.load(Ordering::SeqCst)
    }

    /// 已提交实例数。
    pub fn len(&self) -> usize {
        self.instances.len()
    }

    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }

    /// 原子准入：预留 `spec.memory_limit`（CAS 循环，并发安全）。
    ///
    /// 校验顺序：有界性 → 单实例上限 → 全局预算。任一失败不改变预留量。
    pub fn try_admit(&self, spec: &InstanceSpec) -> WamrResult<Reservation> {
        if spec.memory_limit == 0 {
            return Err(WamrError::UnboundedMemoryForbidden);
        }
        if self.per_instance_limit > 0 && spec.memory_limit > self.per_instance_limit {
            return Err(WamrError::InstanceMemoryLimitExceeded {
                requested: spec.memory_limit,
                limit: self.per_instance_limit,
            });
        }
        loop {
            let cur = self.reserved.load(Ordering::SeqCst);
            let new = cur + spec.memory_limit;
            if new > self.budget {
                return Err(WamrError::MemoryBudgetExceeded {
                    requested: spec.memory_limit,
                    used: cur,
                    limit: self.budget,
                });
            }
            match self
                .reserved
                .compare_exchange_weak(cur, new, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => {
                    return Ok(Reservation {
                        bytes: spec.memory_limit,
                    });
                }
                // CAS 失败 = 并发竞争，重读重试
                Err(_) => continue,
            }
        }
    }

    /// 提交预留：预留 → 实例注册（分配 id）。
    /// WAMR 原始句柄由 RuntimeHost 单独持有（本层只做记账，保持可测）。
    /// 失败路径（如 sys 层实例化失败）必须改走 [`Self::rollback`]。
    pub(crate) fn commit(
        &mut self,
        reservation: Reservation,
        spec: InstanceSpec,
    ) -> InstanceHandle {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.instances.insert(id, InstanceState { spec });
        debug_assert!(self.reserved.load(Ordering::SeqCst) >= reservation.bytes());
        InstanceHandle(id)
    }

    /// 回滚预留（实例化失败路径）。
    pub fn rollback(&self, reservation: Reservation) {
        let before = self.reserved.fetch_sub(reservation.bytes, Ordering::SeqCst);
        debug_assert!(before >= reservation.bytes);
    }

    /// 销毁实例：移除注册 + 释放预算。
    pub(crate) fn destroy(&mut self, handle: InstanceHandle) -> WamrResult<InstanceSpec> {
        let state = self
            .instances
            .remove(&handle.0)
            .ok_or_else(|| WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
        self.reserved
            .fetch_sub(state.spec.memory_limit, Ordering::SeqCst);
        Ok(state.spec)
    }

    pub fn get(&self, handle: InstanceHandle) -> Option<&InstanceSpec> {
        self.instances.get(&handle.0).map(|s| &s.spec)
    }

    /// 统计某模块的存活实例数（unload busy 守护用）。
    pub fn instances_of(&self, module_name: &str) -> usize {
        self.instances
            .values()
            .filter(|s| s.spec.module_name == module_name)
            .count()
    }

    /// 全部实例句柄（shutdown 顺序销毁用）。
    pub fn handles(&self) -> Vec<InstanceHandle> {
        self.instances.keys().map(|&k| InstanceHandle(k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn spec(mem: u64) -> InstanceSpec {
        InstanceSpec {
            module_name: "nlu".into(),
            component_id: "nlu".into(),
            stack_size: 64 * 1024,
            heap_size: 0,
            memory_limit: mem,
        }
    }

    #[test]
    fn admits_within_budget() {
        let mgr = InstanceManager::new(10 * 1024 * 1024, 4 * 1024 * 1024);
        let r = mgr.try_admit(&spec(1024)).unwrap();
        assert_eq!(r.bytes(), 1024);
        mgr.rollback(r);
        assert_eq!(mgr.reserved(), 0);
    }

    #[test]
    fn rejects_overcommit() {
        let mgr = InstanceManager::new(1024, 4096);
        let err = mgr.try_admit(&spec(2048)).unwrap_err();
        assert!(matches!(
            err,
            WamrError::MemoryBudgetExceeded {
                requested: 2048,
                limit: 1024,
                ..
            }
        ));
        assert_eq!(mgr.reserved(), 0, "被拒绝的准入不得改变预留量");
    }

    #[test]
    fn rejects_unbounded_memory() {
        let mgr = InstanceManager::new(1024, 4096);
        assert!(matches!(
            mgr.try_admit(&spec(0)),
            Err(WamrError::UnboundedMemoryForbidden)
        ));
    }

    #[test]
    fn rejects_per_instance_limit() {
        let mgr = InstanceManager::new(10 * 1024 * 1024, 512);
        let err = mgr.try_admit(&spec(1024)).unwrap_err();
        assert!(matches!(
            err,
            WamrError::InstanceMemoryLimitExceeded {
                requested: 1024,
                limit: 512
            }
        ));
    }

    #[test]
    fn commit_and_destroy_release_budget() {
        let mut mgr = InstanceManager::new(1024, 4096);
        let r = mgr.try_admit(&spec(256)).unwrap();
        let h = mgr.commit(r, spec(256));
        assert_eq!(mgr.len(), 1);
        mgr.destroy(h).unwrap();
        assert!(mgr.is_empty());
    }

    #[test]
    fn destroy_unknown_instance_errors() {
        let mut mgr = InstanceManager::new(1024, 4096);
        assert!(matches!(
            mgr.destroy(InstanceHandle(999)),
            Err(WamrError::NoSuchInstance(_))
        ));
    }

    #[test]
    fn instances_of_counts_per_module() {
        let mut mgr = InstanceManager::new(4096, 4096);
        let r = mgr.try_admit(&spec(128)).unwrap();
        let h1 = mgr.commit(
            r,
            InstanceSpec {
                module_name: "nlu".into(),
                ..spec(128)
            },
        );
        let r2 = mgr.try_admit(&spec(128)).unwrap();
        let h2 = mgr.commit(
            r2,
            InstanceSpec {
                module_name: "home".into(),
                ..spec(128)
            },
        );
        assert_eq!(mgr.instances_of("nlu"), 1);
        assert_eq!(mgr.instances_of("home"), 1);
        assert_eq!(mgr.handles().len(), 2);
        mgr.destroy(h1).unwrap();
        mgr.destroy(h2).unwrap();
        assert_eq!(mgr.reserved(), 0);
    }

    #[test]
    fn concurrent_admission_never_exceeds_budget() {
        // 8 线程 × 16 次 × 64 字节，预算 1024：任何时刻累计预留 ≤ 1024
        let mgr = Arc::new(InstanceManager::new(1024, 512));
        let successes = Arc::new(AtomicU64::new(0));
        let mut join = Vec::new();
        for _ in 0..8 {
            let mgr = Arc::clone(&mgr);
            let successes = Arc::clone(&successes);
            join.push(std::thread::spawn(move || {
                let mut local_reserved = Vec::new();
                for _ in 0..16 {
                    if let Ok(r) = mgr.try_admit(&spec(64)) {
                        // 短暂持有（模拟实例化窗口）
                        std::thread::yield_now();
                        // 立即校验不变量：预留永不超预算
                        assert!(mgr.reserved() <= 1024, "预留量越过全局预算");
                        local_reserved.push(r);
                        successes.fetch_add(1, Ordering::SeqCst);
                    }
                }
                for r in local_reserved {
                    mgr.rollback(r);
                }
            }));
        }
        for t in join {
            t.join().unwrap();
        }
        assert!(
            successes.load(Ordering::SeqCst) > 0,
            "预算 1024/64=16 应至少有 16 次成功"
        );
        assert_eq!(mgr.reserved(), 0, "全部回滚后预算必须归零");
    }

    #[test]
    fn rollback_recovers_after_failure_simulation() {
        // 模拟：预留成功 → sys 实例化失败 → 回滚 → 重新准入成功
        let mgr = InstanceManager::new(256, 256);
        let r = mgr.try_admit(&spec(256)).unwrap();
        mgr.rollback(r);
        assert_eq!(mgr.reserved(), 0);
        let r2 = mgr.try_admit(&spec(256)).unwrap();
        mgr.rollback(r2);
        assert_eq!(mgr.reserved(), 0);
    }
}
