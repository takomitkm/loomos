//! 类型化 ID（architecture/09 清单 D 第一条：Resource ID、Component ID、Execution ID、Operation ID）。
//!
//! 纪律：ID 一律 Newtype，不导出裸 `u64` 构造。`define_id!` 只在本模块内使用，
//! 对外暴露的具体类型有明确的构造入口，避免「随便一个数字」冒充身份。

use serde::{Deserialize, Serialize};
use std::fmt;

use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(1);

fn next_raw() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

macro_rules! define_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(u64);

        impl $name {
            /// 分配一个新 ID。这是唯一的构造入口。
            pub fn next() -> Self {
                Self(next_raw())
            }
            /// 从持久化数据恢复 ID（反序列化以外不对外提供）。
            pub fn from_raw(v: u64) -> Option<Self> {
                (v != 0).then_some(Self(v))
            }
            pub fn as_raw(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "-{}"), self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }
    };
}

define_id!(
    /// 组件身份（Manifest 声明的是名字，运行实例另有此 ID）。
    ComponentId,
    "comp"
);
define_id!(
    /// 资源身份（租约管理器的资源条目）。
    ResourceId,
    "res"
);
define_id!(
    /// 一份租约的标识。
    LeaseId,
    "lease"
);
define_id!(
    /// 一笔异步操作的标识（devices/inference 共用同一语义，见 architecture/06 §6.4）。
    OperationId,
    "op"
);
define_id!(
    /// 一次执行上下文的标识。
    ExecutionId,
    "exec"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_display_prefixed() {
        let a = OperationId::next();
        let b = OperationId::next();
        assert_ne!(a, b);
        assert!(a.as_raw() > 0);
        assert_eq!(a.to_string(), format!("op-{}", a.as_raw()));
        assert!(LeaseId::from_raw(0).is_none());
        assert!(LeaseId::from_raw(7).is_some());
    }

    #[test]
    fn serde_roundtrip() {
        let id = ResourceId::next();
        let json = serde_json::to_string(&id).unwrap();
        let back: ResourceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }
}
