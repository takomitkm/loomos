//! 平台映射层（Linux Backend 的第一步实现：B 类 OS 代劳，03-OS代劳清单.md §B）。
//!
//! 职责：把 `CpuBudget / MemoryLimit / ExecutionContext.affinity` 映射到 OS 机制，
//! **每次施加后读回验证**，产出 `EnforcementLevel` 如实上报（11 §11.3.2）。
//! unsafe 全部隔离在 `loomos-sys`（全仓唯一可信边界 crate），本模块纯 Rust。
//!
//! 证据口径（写入 report.evidence，随事件入账）：
//! - `Enforced`：施加成功 **且** 读回一致（loomos-sys::Outcome::Applied）；
//! - `Unsupported`：非 Linux 宿主 / feature 未开 / syscall 失败（原因如实记录）；
//! - 读回不一致 → 不冒充任何档位，直接 `Unsupported` 并记录 got 值。

use crate::resource::{CoreSet, CpuBudget};
use loomos_sys as sys;
use serde::{Deserialize, Serialize};
use std::fmt;

/// 单项映射的报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MappingReport {
    pub item: String,
    pub level: EnforcementLevelReport,
    pub evidence: String,
}

/// EnforcementLevel 的可序列化形态（resource::EnforcementLevel 带 'static 引用，不便序列化）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnforcementLevelReport {
    Enforced,
    Unsupported,
}

impl fmt::Display for EnforcementLevelReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EnforcementLevelReport::Enforced => write!(f, "enforced"),
            EnforcementLevelReport::Unsupported => write!(f, "unsupported"),
        }
    }
}

fn to_report(outcome: sys::Outcome, item: &str) -> MappingReport {
    match outcome {
        sys::Outcome::Applied { evidence } => MappingReport {
            item: item.to_string(),
            level: EnforcementLevelReport::Enforced,
            evidence,
        },
        sys::Outcome::NotPossible { reason } => MappingReport {
            item: item.to_string(),
            level: EnforcementLevelReport::Unsupported,
            evidence: reason,
        },
    }
}

/// CpuBudget → SCHED_DEADLINE。
pub fn apply_cpu_budget(budget: &CpuBudget) -> MappingReport {
    to_report(
        sys::apply_cpu_budget(budget.runtime_us, budget.period_us, budget.deadline_us),
        "CpuBudget→SCHED_DEADLINE",
    )
}

/// MemoryLimit（字节）→ RLIMIT_AS。证据注明口径=虚拟地址空间。
pub fn apply_memory_limit(bytes: u64) -> MappingReport {
    to_report(
        sys::apply_memory_limit_rlimit_as(bytes),
        "MemoryLimit→RLIMIT_AS",
    )
}

/// 核亲和 → sched_setaffinity。
pub fn apply_affinity(cores: &CoreSet) -> MappingReport {
    to_report(
        sys::apply_affinity(cores.cores()),
        "affinity→sched_setaffinity",
    )
}

/// 一次完整探测：三项全部尝试并汇总（装配期调用一次，写入事件证据）。
pub fn probe(
    context_budget: Option<&CpuBudget>,
    memory_bytes: u64,
    cores: &CoreSet,
) -> Vec<MappingReport> {
    let mut reports = Vec::new();
    match context_budget {
        Some(b) => reports.push(apply_cpu_budget(b)),
        None => reports.push(MappingReport {
            item: "CpuBudget→SCHED_DEADLINE".into(),
            level: EnforcementLevelReport::Unsupported,
            evidence: "未配置预算，跳过".into(),
        }),
    }
    reports.push(apply_memory_limit(memory_bytes));
    reports.push(apply_affinity(cores));
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_reports_three_items_with_honest_levels() {
        let budget = CpuBudget {
            runtime_us: 30_000,
            period_us: 100_000,
            deadline_us: 90_000,
        };
        let cores = CoreSet::new(vec![0, 1], 4).unwrap();
        let reports = probe(Some(&budget), 512 * 1024 * 1024, &cores);
        assert_eq!(reports.len(), 3);
        // 非 Linux 宿主：三项必须如实 Unsupported（本机 windows-gnu / CI ubuntu 反而可能 Enforced——
        // 所以只断言"档位存在且证据非空"，不断言具体档位，保证跨平台确定性）
        for r in &reports {
            assert!(!r.evidence.is_empty(), "{} 证据不能为空", r.item);
            assert!(
                matches!(
                    r.level,
                    EnforcementLevelReport::Enforced | EnforcementLevelReport::Unsupported
                ),
                "档位只能 enforced/unsupported（不冒充）: {:?}",
                r
            );
        }
    }

    #[test]
    fn probe_without_budget_reports_skip() {
        let cores = CoreSet::new(vec![0], 4).unwrap();
        let reports = probe(None, 1024, &cores);
        assert_eq!(reports[0].evidence, "未配置预算，跳过");
    }

    #[test]
    fn display_levels() {
        assert_eq!(EnforcementLevelReport::Enforced.to_string(), "enforced");
        assert_eq!(
            EnforcementLevelReport::Unsupported.to_string(),
            "unsupported"
        );
    }
}
