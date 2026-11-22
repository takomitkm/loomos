//! 共享持久数据 + 迁移纪律（ADR-013 §决定 4）。
//!
//! **不可逆迁移必须等新槽被确认 successful 之后才允许执行**；升级期间的
//! 共享数据写法必须保持旧槽可读（回滚兼容）。本模块用类型状态把这条纪律
//! 编进 API：未确认的槽拿不到 `write_migration` 的授权。

use crate::slot::SlotId;

/// 共享数据区（/var/lib/loomos 等的抽象）。
#[derive(Debug, Default)]
pub struct SharedData {
    /// schema 版本（应用层演进标识）。
    schema_version: u32,
    /// 已执行的不可逆迁移记录（确认后追加，回滚审计用）。
    applied_migrations: Vec<(u32, String)>,
}

impl SharedData {
    pub fn new(schema_version: u32) -> Self {
        Self {
            schema_version,
            applied_migrations: Vec::new(),
        }
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn applied_migrations(&self) -> &[(u32, String)] {
        &self.applied_migrations
    }

    /// 申请执行不可逆迁移。
    ///
    /// `confirmed_slot` 必须是**已被确认 successful 的槽**——未确认时拒绝
    /// （类型状态由调用方拿到的 `SlotConfirmation` 凭证表达）。
    pub fn apply_irreversible_migration(
        &mut self,
        target_version: u32,
        description: &str,
        confirmation: &SlotConfirmation,
    ) -> Result<(), crate::AbError> {
        if self.schema_version >= target_version {
            return Err(crate::AbError::Migration(format!(
                "schema 已是 v{}，拒绝重复迁移到 v{target_version}",
                self.schema_version
            )));
        }
        self.schema_version = target_version;
        self.applied_migrations
            .push((target_version, description.to_string()));
        tracing::info!(slot = ?confirmation.slot(), to = target_version, "不可逆迁移已执行");
        Ok(())
    }

    /// 升级期间的兼容写（旧槽可读——回滚兼容契约由实现方保证，此处只记账）。
    pub fn compatible_write(&mut self, key: &str, value: &str) {
        tracing::debug!(key, "兼容写（旧槽可读纪律由应用层 schema 保证）");
        let _ = (key, value);
    }
}

/// 槽位确认凭证（健康检查全过、`SlotTable::confirm` 之后由事务签发）。
///
/// **凭证只能从确认路径获得**——这是「不可逆迁移必须在新槽确认后」纪律的
/// 类型级表达（AGENTS.md：TypeChecked 级证据）。
#[derive(Debug, Clone, Copy)]
pub struct SlotConfirmation {
    slot: SlotId,
}

impl SlotConfirmation {
    /// 仅由事务层在健康检查通过后签发（crate 内可见）。
    pub(crate) fn new(slot: SlotId) -> Self {
        Self { slot }
    }

    pub fn slot(&self) -> SlotId {
        self.slot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_requires_confirmation_token() {
        let mut data = SharedData::new(1);
        // 没有凭证就无法调用 apply——类型层面阻止（编译期）；运行期路径也验证：
        let conf = SlotConfirmation::new(SlotId::B);
        data.apply_irreversible_migration(2, "v2: 事件表重建", &conf)
            .unwrap();
        assert_eq!(data.schema_version(), 2);
        assert_eq!(data.applied_migrations().len(), 1);
    }

    #[test]
    fn duplicate_migration_rejected() {
        let mut data = SharedData::new(2);
        let conf = SlotConfirmation::new(SlotId::A);
        assert!(data.apply_irreversible_migration(2, "重复", &conf).is_err());
        // 降级迁移（目标 < 当前）也拒绝——回滚后旧槽不得被新 schema 污染
        assert!(data.apply_irreversible_migration(1, "降级", &conf).is_err());
    }
}
