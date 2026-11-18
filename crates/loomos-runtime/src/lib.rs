//! loomos 系统运行时。
//!
//! 对应 `docs/dev/architecture/03-os-runtime.md` 的模块划分：
//! ids / resource / capability / configuration / component / supervisor / recovery / event。
//!
//! 设计纪律（docs/dev/engineering/01-总体设计纲要.md）：
//! - 身份用 Newtype，禁止裸 `u64` 在模块间传递；
//! - 资源不使用万能操作接口，本 crate 只统一身份/权限/生命周期/契约；
//! - 实施等级必须如实上报（enforced / best_effort / unsupported），
//!   不得把未强制实施的约束报告为 enforced（architecture/11 §11.3.2）。

pub mod capability;
pub mod component;
pub mod config;
pub mod event;
pub mod ids;
pub mod resource;
pub mod supervisor;

pub use capability::{CapabilityError, CapabilityId, CapabilityStore, Rights, Token};
pub use component::{ComponentError, ComponentManifest, LifecycleManager, LifecycleState};
pub use config::{
    ConfigError, ConfigService, ConfigTransaction, ConfigValue, NamespacePath, Schema, SchemaField,
};
pub use event::{Event, EventKind, EventLog};
pub use ids::{ComponentId, ExecutionId, LeaseId, OperationId, ResourceId};
pub use resource::{
    CoreSet, CpuBudget, EnforcementLevel, ExecutionContext, ExecutionContract, Lease, LeaseManager,
    LeaseMode, LeaseState, ResourceError, ResourceKind,
};
pub use supervisor::{FailureClass, RecoveryPolicy, Supervisor};

/// 运行时版本，写入事件与证据报告，保证证据可追溯到代码状态。
pub const RUNTIME_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 测试辅助（仅测试构建）：唯一临时目录（不引 tempfile，避免 windows-gnu 上的
/// raw-dylib/dlltool 依赖链）。
#[cfg(test)]
pub(crate) mod testutil {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQ: AtomicU64 = AtomicU64::new(0);

    pub fn tempdir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("loomos-test-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }
}
