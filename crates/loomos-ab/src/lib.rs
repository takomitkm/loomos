//! loomos-ab —— A/B 系统更新**状态机与事务框架**（ADR-013 / architecture/16）。
//!
//! 职责边界（本 crate 是**自动化可验证的事务逻辑层**）：
//! - 双槽模型（SlotState: Good/Bad/Trying）+ boot 计数回退；
//! - 更新事务 13 步的状态机（非法转换显式拒绝——负测试锁定）；
//! - bundle 清单（RAUC 兼容字段子集）+ 校验器 trait（签名/校验和）；
//! - 共享数据迁移纪律：**新槽未确认前禁止不可逆迁移**（ADR-013 §决定 4）；
//! - 引导控制抽象 [`BootControl`]——真实 bootloader（U-Boot bootenv/bootcount）
//!   是板端适配（P5，Unverified），本层用 mock/模拟器测全部回滚路径。
//!
//! **不做**：真实分区写入、真实 RAUC 进程调用、板端 bootenv 语义——
//! 那些是 L2 板测（不进无人值守 CI，AGENTS.md 纪律）。
//!
//! 测试矩阵对应 ADR-013 §16.4：成功更新/中断/非法签名/校验和不符/板型不匹配/
//! 新槽引导失败/健康检查失败/计数上限回退/迁移安全拒绝/旧镜像完好。

#![forbid(unsafe_code)]

pub mod boot;
pub mod bundle;
pub mod data;
pub mod slot;
pub mod tx;

pub use boot::{BootControl, MockBoot};
pub use bundle::BundleManifest;
pub use data::SharedData;
pub use slot::{SlotId, SlotState, SlotTable};
pub use tx::{TxState, UpdateTx};

/// A/B 层错误。
#[derive(Debug, thiserror::Error)]
pub enum AbError {
    /// 非法状态转换（状态机负测试锁定）。
    #[error("非法状态转换: {from:?} → {to:?}")]
    InvalidTransition {
        from: crate::tx::TxState,
        to: &'static str,
    },
    /// bundle 校验失败（签名/校验和/板型）。
    #[error("bundle 校验失败: {0}")]
    VerificationFailed(String),
    /// slot 状态错误。
    #[error("槽位状态错误: {0}")]
    Slot(String),
    /// 共享数据迁移纪律违反。
    #[error("数据迁移纪律违反: {0}")]
    Migration(String),
    /// 引导层错误。
    #[error("引导控制错误: {0}")]
    Boot(String),
}

pub type AbResult<T> = Result<T, AbError>;
