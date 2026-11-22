//! Wasm 实例管理：实例化、执行环境创建、内存/栈预算。

use crate::error::{WamrError, WamrResult};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct InstanceSpec {
    pub module_name: String,
    /// 实例栈大小（字节；覆盖 RuntimeConfig 默认值）。
    pub stack_size: u32,
    /// 实例线性内存上限（字节；0 = 不限）。
    pub memory_limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceHandle(pub u64);

/// 实例管理器：注册/查找/预算检查。
pub struct InstanceManager {
    instances: BTreeMap<u64, InstanceSpec>,
    next_id: u64,
    /// 全局内存预算（所有实例累计）。
    pub total_memory_budget: u64,
    pub total_memory_used: u64,
}

impl InstanceManager {
    pub fn new(total_memory_budget: u64) -> Self {
        Self {
            instances: BTreeMap::new(),
            next_id: 1,
            total_memory_budget,
            total_memory_used: 0,
        }
    }

    /// 准入检查：实例内存请求是否在全局预算内。
    pub fn check_memory(&self, spec: &InstanceSpec) -> WamrResult<()> {
        let limit = self.total_memory_budget;
        if spec.memory_limit > 0 && self.total_memory_used + spec.memory_limit > limit {
            return Err(WamrError::MemoryBudgetExceeded {
                requested: spec.memory_limit,
                limit,
            });
        }
        Ok(())
    }

    pub fn register(&mut self, spec: InstanceSpec) -> InstanceHandle {
        let id = self.next_id;
        self.next_id += 1;
        self.instances.insert(id, spec);
        InstanceHandle(id)
    }

    pub fn remove(&mut self, handle: InstanceHandle) -> WamrResult<()> {
        self.instances
            .remove(&handle.0)
            .ok_or(WamrError::NoSuchInstance(format!("inst-{}", handle.0)))?;
        Ok(())
    }

    pub fn get(&self, handle: InstanceHandle) -> Option<&InstanceSpec> {
        self.instances.get(&handle.0)
    }
}
