//! loomos 唯一可信边界 crate：OS 映射的 unsafe 封装。
//!
//! 本 crate 是**全仓库唯一允许 `unsafe` 的地方**（工程纲要 02：
//! 「unsafe 集中在少量 FFI/系统模块」；其余 11 个 crate 均 `#![forbid(unsafe_code)]`）。
//!
//! 提供 B 类 OS 映射（docs/dev/engineering/03-OS代劳清单.md §B）：
//! - CpuBudget → `sched_setattr(SCHED_DEADLINE)` + 读回；
//! - MemoryLimit → `setrlimit(RLIMIT_AS)` + 读回；
//! - 核亲和 → `sched_setaffinity` + 读回。
//!
//! 纪律（architecture/11 §11.3.2）：
//! - 每次申请后**读回验证**，读回一致才允许上报 enforced；
//! - 「接口存在 ≠ 能力生效」（systemd 假闸门教训）；
//! - 非 Linux 目标 / feature 未开 → 全部 `Unsupported` 桩，不假装成功。

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(
    unsafe_code,
    reason = "全仓唯一可信边界：OS 系统调用映射，SAFETY 逐条注释"
)]

/// 一次映射的结果：要么"已施加且读回一致"（附证据），要么"不可能"（附原因）。
/// 没有"大概成功"这一档——读不回就不承认（11 §11.3.2 举证要求）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied {
        /// 读回证据（如 "sched_getattr: runtime=30000us period=100000us deadline=90000us"）。
        evidence: String,
    },
    NotPossible {
        reason: String,
    },
}

/// 核集合 → 内核 128 字节位图（1024 核，与 `cpu_set_t` 布局一致）。
/// 纯 Rust，跨平台可测。
pub fn coreset_to_mask(cores: &[u32]) -> Result<[u8; 128], String> {
    let mut mask = [0u8; 128];
    for &c in cores {
        if c >= 1024 {
            return Err(format!("核 {c} 越界（位图容量 1024）"));
        }
        mask[(c / 8) as usize] |= 1 << (c % 8);
    }
    Ok(mask)
}

/// 位图 → 核集合（读回比对用）。
pub fn mask_to_cores(mask: &[u8; 128]) -> Vec<u32> {
    let mut out = Vec::new();
    for (byte_idx, b) in mask.iter().enumerate() {
        for bit in 0..8 {
            if b & (1 << bit) != 0 {
                out.push((byte_idx * 8 + bit) as u32);
            }
        }
    }
    out
}

// ---------------- 非 Linux / feature 未开：Unsupported 桩 ----------------

/// CpuBudget → SCHED_DEADLINE。
#[cfg(not(all(target_os = "linux", feature = "linux-backend")))]
pub fn apply_cpu_budget(_runtime_us: u64, _period_us: u64, _deadline_us: u64) -> Outcome {
    Outcome::NotPossible {
        reason: "非 Linux 宿主或 linux-backend feature 未启用".into(),
    }
}

/// MemoryLimit → RLIMIT_AS。
#[cfg(not(all(target_os = "linux", feature = "linux-backend")))]
pub fn apply_memory_limit_rlimit_as(_bytes: u64) -> Outcome {
    Outcome::NotPossible {
        reason: "非 Linux 宿主或 linux-backend feature 未启用".into(),
    }
}

/// 核亲和 → sched_setaffinity。
#[cfg(not(all(target_os = "linux", feature = "linux-backend")))]
pub fn apply_affinity(cores: &[u32]) -> Outcome {
    // 位图换算仍然执行：纯逻辑可测；结果如实报 Unsupported
    match coreset_to_mask(cores) {
        Ok(_) => Outcome::NotPossible {
            reason: "非 Linux 宿主或 linux-backend feature 未启用".into(),
        },
        Err(e) => Outcome::NotPossible { reason: e },
    }
}

// ---------------- Linux 实现（cfg 门控） ----------------

#[cfg(all(target_os = "linux", feature = "linux-backend"))]
mod linux {
    //! SAFETY 总则：所有 `unsafe` 都是单线程上下文内的直接系统调用包装，
    //! 参数由本 crate 构造、无别名指针；每次调用后立即读回验证。

    use super::Outcome;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SchedAttr {
        size: u32,
        sched_policy: u32,
        sched_flags: u64,
        sched_nice: i32,
        sched_priority: u32,
        sched_runtime: u64,
        sched_deadline: u64,
        sched_period: u64,
    }

    const SCHED_DEADLINE: u32 = 6;
    // syscall 号：aarch64 与 x86_64 相同（314/315）
    const SYS_SCHED_SETATTR: libc::c_long = 314;
    const SYS_SCHED_GETATTR: libc::c_long = 315;

    /// CpuBudget → SCHED_DEADLINE，读回验证。
    pub fn apply_cpu_budget(runtime_us: u64, period_us: u64, deadline_us: u64) -> Outcome {
        let attr = SchedAttr {
            size: std::mem::size_of::<SchedAttr>() as u32,
            sched_policy: SCHED_DEADLINE,
            sched_flags: 0,
            sched_nice: 0,
            sched_priority: 0,
            sched_runtime: runtime_us * 1000,
            sched_deadline: deadline_us * 1000,
            sched_period: period_us * 1000,
        };
        // SAFETY: attr 为本函数构造的合法 repr(C) 结构，size 正确；pid 0 = 当前线程。
        let ret = unsafe { libc::syscall(SYS_SCHED_SETATTR, 0, &attr as *const SchedAttr, 0u32) };
        if ret != 0 {
            let errno = std::io::Error::last_os_error();
            return Outcome::NotPossible {
                reason: format!(
                    "sched_setattr(SCHED_DEADLINE) 失败: {errno}（需 CAP_SYS_NICE/root）"
                ),
            };
        }
        // 读回验证（11 §11.3.2：设置后读回才可报 enforced）
        let mut got = SchedAttr {
            size: std::mem::size_of::<SchedAttr>() as u32,
            sched_policy: 0,
            sched_flags: 0,
            sched_nice: 0,
            sched_priority: 0,
            sched_runtime: 0,
            sched_deadline: 0,
            sched_period: 0,
        };
        // SAFETY: 缓冲区与 size 均合法。
        let rret = unsafe {
            libc::syscall(
                SYS_SCHED_GETATTR,
                0,
                &mut got as *mut SchedAttr,
                got.size,
                0u32,
            )
        };
        if rret != 0 {
            return Outcome::NotPossible {
                reason: format!(
                    "sched_getattr 读回失败: {}",
                    std::io::Error::last_os_error()
                ),
            };
        }
        let matches = got.sched_policy == SCHED_DEADLINE
            && got.sched_runtime == runtime_us * 1000
            && got.sched_period == period_us * 1000
            && got.sched_deadline == deadline_us * 1000;
        if matches {
            Outcome::Applied {
                evidence: format!(
                    "sched_getattr: policy=DEADLINE runtime={runtime_us}us period={period_us}us deadline={deadline_us}us（读回一致）"
                ),
            }
        } else {
            Outcome::NotPossible {
                reason: format!(
                    "读回不一致：got policy={} runtime={} period={} deadline={}",
                    got.sched_policy, got.sched_runtime, got.sched_period, got.sched_deadline
                ),
            }
        }
    }

    /// MemoryLimit → RLIMIT_AS，读回验证。
    /// 注意语义：RLIMIT_AS 限制**虚拟地址空间**（含保留），与 RSS 有差距——
    /// 证据里必须写明口径（11 §11.3.2"不得混为一谈"）。
    pub fn apply_memory_limit_rlimit_as(bytes: u64) -> Outcome {
        let lim = libc::rlimit {
            rlim_cur: bytes,
            rlim_max: bytes,
        };
        // SAFETY: lim 为栈上合法结构。
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &lim) };
        if ret != 0 {
            return Outcome::NotPossible {
                reason: format!(
                    "setrlimit(RLIMIT_AS) 失败: {}",
                    std::io::Error::last_os_error()
                ),
            };
        }
        let mut got = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        // SAFETY: 合法指针。
        let rret = unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut got) };
        if rret != 0 {
            return Outcome::NotPossible {
                reason: "getrlimit 读回失败".into(),
            };
        }
        if got.rlim_cur == bytes {
            Outcome::Applied {
                evidence: format!(
                    "getrlimit(RLIMIT_AS)={bytes} 字节（读回一致；口径=虚拟地址空间，非 RSS）"
                ),
            }
        } else {
            Outcome::NotPossible {
                reason: format!("读回不一致：got rlim_cur={}", got.rlim_cur),
            }
        }
    }

    /// 核亲和 → sched_setaffinity(pid 0)，读回验证。
    pub fn apply_affinity(cores: &[u32]) -> Outcome {
        let mask = match super::coreset_to_mask(cores) {
            Ok(m) => m,
            Err(e) => return Outcome::NotPossible { reason: e },
        };
        // SAFETY: 128 字节位图与 kernel cpumask 布局一致；pid 0 = 当前线程。
        let ret = unsafe {
            libc::sched_setaffinity(0, mask.len(), mask.as_ptr() as *const libc::cpu_set_t)
        };
        if ret != 0 {
            return Outcome::NotPossible {
                reason: format!(
                    "sched_setaffinity 失败: {}",
                    std::io::Error::last_os_error()
                ),
            };
        }
        let mut got = [0u8; 128];
        // SAFETY: 合法缓冲区。
        let rret = unsafe {
            libc::sched_getaffinity(0, got.len(), got.as_mut_ptr() as *mut libc::cpu_set_t)
        };
        if rret != 0 {
            return Outcome::NotPossible {
                reason: "sched_getaffinity 读回失败".into(),
            };
        }
        let got_cores = super::mask_to_cores(&got);
        if got_cores == cores {
            Outcome::Applied {
                evidence: format!("sched_getaffinity={got_cores:?}（读回一致）"),
            }
        } else {
            Outcome::NotPossible {
                reason: format!("读回不一致：want {cores:?} got {got_cores:?}"),
            }
        }
    }
}

#[cfg(all(target_os = "linux", feature = "linux-backend"))]
pub use linux::{apply_affinity, apply_cpu_budget, apply_memory_limit_rlimit_as};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coreset_mask_roundtrip() {
        let mask = coreset_to_mask(&[0, 1, 3]).unwrap();
        assert_eq!(mask_to_cores(&mask), vec![0, 1, 3]);
        // 位图字节正确性：核 3 → 第 0 字节 bit3
        assert_eq!(mask[0], 0b0000_1011);
    }

    #[test]
    fn coreset_out_of_range() {
        assert!(coreset_to_mask(&[1024]).is_err());
        assert!(coreset_to_mask(&[0, 500, 1023]).is_ok());
    }

    #[test]
    #[cfg(not(all(target_os = "linux", feature = "linux-backend")))]
    // 仅桩路径存在时运行（非 Linux 目标 或 linux-backend 未启用）——
    // linux-backend 开启且 Linux 时走真实实现（Applied），不是桩，不应跑此断言
    fn stub_reports_unsupported_off_linux() {
        // 非 Linux 目标（含本机 windows-gnu）：桩必须如实报 Unsupported
        let o = apply_cpu_budget(30_000, 100_000, 90_000);
        assert!(matches!(o, Outcome::NotPossible { .. }));
        let o2 = apply_affinity(&[0, 1]);
        assert!(matches!(o2, Outcome::NotPossible { .. }));
    }
}
