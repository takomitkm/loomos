//! 双槽模型：A/B 槽位状态 + 引导计数。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// 槽位标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SlotId {
    A,
    B,
}

impl SlotId {
    /// 另一个槽（A/B 互为对侧）。
    pub fn other(self) -> Self {
        match self {
            SlotId::A => SlotId::B,
            SlotId::B => SlotId::A,
        }
    }
}

/// 槽位健康状态（RAUC 语义：good/bad）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotState {
    /// 已确认可引导（上次启动健康检查通过）。
    Good,
    /// 标记不可引导（健康检查失败/计数耗尽）。
    Bad,
    /// 试引导中（更新后未确认；计数范围内）。
    Trying,
}

/// 单槽状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Slot {
    pub state: SlotState,
    /// 已引导尝试次数（Trying 状态下递增）。
    pub boot_attempts: u32,
    /// 槽内系统镜像指纹（旧镜像完好性验证用）。
    pub image_digest: Option<String>,
}

/// 双槽表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotTable {
    slots: BTreeMap<SlotId, Slot>,
    /// 当前活动槽（最后一次被确认 Good 的槽）。
    pub active: SlotId,
    /// boot 计数上限（Try 状态允许的最大尝试；超过即回退——ADR-013 步骤 13）。
    pub max_boot_attempts: u32,
}

impl SlotTable {
    /// 初始装配：active 为 Good，对侧 Good（均带指纹）。
    pub fn initial(active: SlotId, digest_active: &str, digest_other: &str) -> Self {
        let mut slots = BTreeMap::new();
        slots.insert(
            active,
            Slot {
                state: SlotState::Good,
                boot_attempts: 0,
                image_digest: Some(digest_active.into()),
            },
        );
        slots.insert(
            active.other(),
            Slot {
                state: SlotState::Good,
                boot_attempts: 0,
                image_digest: Some(digest_other.into()),
            },
        );
        Self {
            slots,
            active,
            max_boot_attempts: 3,
        }
    }

    pub fn slot(&self, id: SlotId) -> &Slot {
        &self.slots[&id]
    }

    pub fn slot_mut(&mut self, id: SlotId) -> &mut Slot {
        self.slots.get_mut(&id).expect("槽位必然存在")
    }

    /// 试引导目标槽（更新事务步骤 9：写槽+验证后 arm）。
    pub fn arm(&mut self, id: SlotId) {
        let s = self.slot_mut(id);
        s.state = SlotState::Trying;
        s.boot_attempts = 0;
    }

    /// 记一次引导尝试。返回 Ok(()) = 计数内；Err = 计数耗尽（须回退）。
    pub fn record_boot_attempt(&mut self, id: SlotId) -> Result<(), crate::AbError> {
        let s = self.slot_mut(id);
        if s.state != SlotState::Trying {
            return Err(crate::AbError::Slot(format!("{id:?} 不在 Trying 状态")));
        }
        s.boot_attempts += 1;
        let attempts = s.boot_attempts;
        let limit = self.max_boot_attempts;
        if attempts >= limit {
            Err(crate::AbError::Boot(format!(
                "{id:?} 引导计数耗尽（{attempts}/{limit}）"
            )))
        } else {
            Ok(())
        }
    }

    /// 确认槽位成功（健康检查全过——ADR-013 步骤 12）。
    pub fn confirm(&mut self, id: SlotId) {
        let s = self.slot_mut(id);
        s.state = SlotState::Good;
        s.boot_attempts = 0;
        self.active = id;
        // 对侧保持原状（不自动降级——它仍是可回退目标）
    }

    /// 回退：目标槽标 Bad，活动槽切回对侧（须为 Good）。
    pub fn rollback_to(&mut self, id: SlotId) -> Result<(), crate::AbError> {
        use crate::AbError;
        if self.slot(id).state != SlotState::Good {
            return Err(AbError::Slot(format!(
                "回退目标 {id:?} 不是 Good——旧槽完好性纪律被违反"
            )));
        }
        self.slot_mut(id.other()).state = SlotState::Bad;
        self.slot_mut(id.other()).boot_attempts = 0;
        self.active = id;
        Ok(())
    }

    /// 旧镜像完好性：活动槽指纹在回退后不变（ADR-013 测试矩阵末项）。
    pub fn image_digest(&self, id: SlotId) -> Option<&str> {
        self.slot(id).image_digest.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_and_arm_other_slot() {
        let mut t = SlotTable::initial(SlotId::A, "img-a1", "img-b1");
        assert_eq!(t.active, SlotId::A);
        assert_eq!(t.slot(SlotId::A).state, SlotState::Good);
        t.arm(SlotId::B);
        assert_eq!(t.slot(SlotId::B).state, SlotState::Trying);
        assert_eq!(t.slot(SlotId::B).boot_attempts, 0);
    }

    #[test]
    fn boot_counter_exhaustion_at_limit() {
        let mut t = SlotTable::initial(SlotId::A, "a", "b");
        t.arm(SlotId::B);
        assert!(t.record_boot_attempt(SlotId::B).is_ok()); // 1/3
        assert!(t.record_boot_attempt(SlotId::B).is_ok()); // 2/3
        assert!(t.record_boot_attempt(SlotId::B).is_err()); // 3/3 耗尽
        assert_eq!(t.slot(SlotId::B).boot_attempts, 3);
    }

    #[test]
    fn confirm_switches_active_and_keeps_other() {
        let mut t = SlotTable::initial(SlotId::A, "a2", "b2");
        t.arm(SlotId::B);
        t.confirm(SlotId::B);
        assert_eq!(t.active, SlotId::B);
        assert_eq!(t.slot(SlotId::B).state, SlotState::Good);
        // 对侧 A 未被降级（仍是回退目标）
        assert_eq!(t.slot(SlotId::A).state, SlotState::Good);
        assert_eq!(t.image_digest(SlotId::A), Some("a2"));
    }

    #[test]
    fn rollback_marks_target_bad_and_old_slot_intact() {
        let mut t = SlotTable::initial(SlotId::A, "old-img", "new-img");
        t.arm(SlotId::B);
        t.slot_mut(SlotId::B).image_digest = Some("new-img".into());
        t.rollback_to(SlotId::A).unwrap();
        assert_eq!(t.active, SlotId::A);
        assert_eq!(t.slot(SlotId::B).state, SlotState::Bad);
        assert_eq!(t.image_digest(SlotId::A), Some("old-img"), "旧镜像完好");
    }

    #[test]
    fn rollback_to_bad_slot_rejected() {
        let mut t = SlotTable::initial(SlotId::A, "a", "b");
        t.slot_mut(SlotId::B).state = SlotState::Bad;
        assert!(t.rollback_to(SlotId::B).is_err(), "回退目标必须是 Good");
    }

    #[test]
    fn arm_requires_no_trying_state_pollution() {
        let mut t = SlotTable::initial(SlotId::A, "a", "b");
        t.arm(SlotId::B);
        t.arm(SlotId::B); // 重复 arm 重置计数（重新事务）
        assert_eq!(t.slot(SlotId::B).boot_attempts, 0);
    }
}
