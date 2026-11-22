//! 引导控制抽象：bootloader 的槽位选择/确认语义（U-Boot bootenv/bootcount 对应物）。
//!
//! 板端真实 bootloader 是 P5 板测项（Unverified）；本层只定义 trait + mock，
//! 状态机测试用 [`MockBoot`] 驱动全部回滚路径（ADR-013 §证据边界）。

/// bootloader 槽位控制接口。
pub trait BootControl: Send + Sync {
    /// 设置「试引导」槽（写 bootenv；同时重置 bootcount）。
    fn set_trying(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError>;
    /// 标记槽位成功（写 bootenv successful 标志）。
    fn mark_good(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError>;
    /// 标记槽位不可引导。
    fn mark_bad(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError>;
    /// 当前 bootloader 将引导的槽。
    fn current(&self) -> crate::SlotId;
    /// 读取当前 bootcount（诊断用）。
    fn boot_count(&self) -> u32;
}

/// 模拟 bootloader：状态机测试的引导环境。
#[derive(Debug, Default)]
pub struct MockBoot {
    current: Option<crate::SlotId>,
    count: u32,
    /// 故障注入：set_trying 后引导时是否失败（模拟新槽起不来）。
    pub fail_boot_of: Option<crate::SlotId>,
}

impl BootControl for MockBoot {
    fn set_trying(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError> {
        self.current = Some(slot);
        self.count = 0;
        Ok(())
    }

    fn mark_good(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError> {
        self.current = Some(slot);
        self.count = 0;
        Ok(())
    }

    fn mark_bad(&mut self, slot: crate::SlotId) -> Result<(), crate::AbError> {
        // 标记后 bootloader 回退到对侧
        self.current = Some(slot.other());
        self.count = 0;
        Ok(())
    }

    fn current(&self) -> crate::SlotId {
        self.current.unwrap_or(crate::SlotId::A)
    }

    fn boot_count(&self) -> u32 {
        self.count
    }
}

impl MockBoot {
    /// 故障注入构造（目标槽引导必然失败——回退路径测试用）。
    pub fn failing(slot: crate::SlotId) -> Self {
        Self {
            fail_boot_of: Some(slot),
            ..Default::default()
        }
    }

    /// 模拟一次引导尝试：目标槽带故障注入时失败（计数+1），否则成功。
    pub fn simulate_boot(&mut self) -> Result<crate::SlotId, crate::AbError> {
        let slot = self.current();
        self.count += 1;
        if self.fail_boot_of == Some(slot) {
            Err(crate::AbError::Boot(format!("{slot:?} 引导失败（注入）")))
        } else {
            Ok(slot)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SlotId;

    #[test]
    fn mock_boot_basic_flow() {
        let mut b = MockBoot::default();
        b.set_trying(SlotId::B).unwrap();
        assert_eq!(b.current(), SlotId::B);
        assert_eq!(b.boot_count(), 0);
        assert_eq!(b.simulate_boot().unwrap(), SlotId::B);
        assert_eq!(b.boot_count(), 1);
    }

    #[test]
    fn mock_boot_failure_injection() {
        let mut b = MockBoot {
            fail_boot_of: Some(SlotId::B),
            ..Default::default()
        };
        b.set_trying(SlotId::B).unwrap();
        assert!(b.simulate_boot().is_err());
        assert_eq!(b.boot_count(), 1);
        // 标 Bad 后回退到对侧
        b.mark_bad(SlotId::B).unwrap();
        assert_eq!(b.current(), SlotId::A);
    }
}
