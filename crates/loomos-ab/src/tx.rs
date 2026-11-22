//! 更新事务状态机（ADR-013 §决定 3 的 13 步事务 → 显式状态）。
//!
//! ```text
//! Idle ──begin(验签/兼容)──▶ BundleVerified ──write_slot──▶ WriteVerified
//!        ▲                                       │arm
//!        │bundle 校验失败                          ▼
//!        └──────────────────────────────        Armed
//!                                                │引导尝试（外部重启后 record_boot_result）
//!                                                ├─计数耗尽─▶ RolledBack（旧槽接管）
//!                                                └─引导成功
//!                                                  ▼
//!                                            HealthChecking
//!                                              ├─通过─▶ Successful（签发 SlotConfirmation）
//!                                              └─失败─▶ RolledBack
//! ```
//!
//! 纪律：
//! - **非法转换显式拒绝**（每个入口校验当前状态——负测试锁定）；
//! - 写槽失败/中断：槽表不动（旧槽完好），事务 Failed，重启用新事务重验 bundle；
//! - 回退路径统一走 [`SlotTable::rollback_to`] + [`BootControl::mark_bad`]；
//! - SlotConfirmation 只在 Successful 路径签发（数据迁移纪律的类型级闸门）。

use crate::boot::BootControl;
use crate::bundle::{BundleManifest, BundleVerifier};
use crate::data::{SharedData, SlotConfirmation};
use crate::slot::{SlotId, SlotTable};
use crate::{AbError, AbResult};
use std::fmt;

/// 事务状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxState {
    Idle,
    BundleVerified,
    WriteVerified,
    Armed,
    HealthChecking,
    Successful,
    RolledBack,
    Failed(String),
}

impl fmt::Display for TxState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxState::Idle => write!(f, "Idle"),
            TxState::BundleVerified => write!(f, "BundleVerified"),
            TxState::WriteVerified => write!(f, "WriteVerified"),
            TxState::Armed => write!(f, "Armed"),
            TxState::HealthChecking => write!(f, "HealthChecking"),
            TxState::Successful => write!(f, "Successful"),
            TxState::RolledBack => write!(f, "RolledBack"),
            TxState::Failed(e) => write!(f, "Failed({e})"),
        }
    }
}

/// 写槽抽象：真实 flash 写入由部署层实现（QEMU/模拟器注入中断）。
pub trait SlotWriter {
    /// 写入镜像并回读校验；返回写入摘要。失败 = 写槽中断/介质错误。
    fn write_image(&mut self, image: &[u8]) -> Result<String, String>;
}

/// 更新事务。
pub struct UpdateTx {
    state: TxState,
    device_compatible: String,
    manifest: Option<BundleManifest>,
    image: Option<Vec<u8>>,
    from_slot: SlotId,
    to_slot: SlotId,
}

impl UpdateTx {
    /// 新事务：目标槽 = 活动槽的对侧。
    pub fn new(slots: &SlotTable, device_compatible: &str) -> Self {
        let from_slot = slots.active;
        Self {
            state: TxState::Idle,
            device_compatible: device_compatible.to_string(),
            manifest: None,
            image: None,
            from_slot,
            to_slot: from_slot.other(),
        }
    }

    pub fn state(&self) -> &TxState {
        &self.state
    }

    pub fn to_slot(&self) -> SlotId {
        self.to_slot
    }

    pub fn from_slot(&self) -> SlotId {
        self.from_slot
    }

    fn require(&self, want: &[TxState], next: &'static str) -> AbResult<()> {
        if want
            .iter()
            .any(|w| std::mem::discriminant(w) == std::mem::discriminant(&self.state))
            && want.iter().any(|w| match (w, &self.state) {
                (TxState::Failed(_), TxState::Failed(_)) => true,
                (a, b) => a == b,
            })
        {
            Ok(())
        } else {
            Err(AbError::InvalidTransition {
                from: self.state.clone(),
                to: next,
            })
        }
    }

    /// 步骤 1–6：验签 + 校验和 + 板型兼容（写槽绝不先于校验）。
    pub fn begin(
        &mut self,
        verifier: &dyn BundleVerifier,
        manifest: BundleManifest,
        files: &[(String, Vec<u8>)],
    ) -> AbResult<()> {
        self.require(&[TxState::Idle], "begin")?;
        match verifier.verify(&manifest, &self.device_compatible, files) {
            Ok(()) => {
                // 取 rootfs 镜像（v1 单镜像）
                let img = manifest
                    .images
                    .first()
                    .and_then(|i| {
                        files
                            .iter()
                            .find(|(n, _)| n == &i.filename)
                            .map(|(_, b)| b.clone())
                    })
                    .ok_or_else(|| AbError::VerificationFailed("清单缺 rootfs 镜像".into()))?;
                self.manifest = Some(manifest);
                self.image = Some(img);
                self.state = TxState::BundleVerified;
                Ok(())
            }
            Err(e) => {
                self.state = TxState::Failed(e.to_string());
                Err(e)
            }
        }
    }

    /// 步骤 7–8：写非活动槽 + 回读校验。失败 = 更新中断：槽表不动，事务 Failed。
    pub fn write_slot(&mut self, writer: &mut dyn SlotWriter) -> AbResult<()> {
        self.require(&[TxState::BundleVerified], "write_slot")?;
        let image = self.image.clone().expect("begin 后必有镜像");
        match writer.write_image(&image) {
            Ok(digest) => {
                // 回读校验：写入摘要 ≡ 清单校验和
                let want = self
                    .manifest
                    .as_ref()
                    .expect("begin 后必有清单")
                    .images
                    .first()
                    .expect("begin 已校验非空")
                    .checksum
                    .clone();
                if digest != want {
                    self.state = TxState::Failed(format!("回读校验不符: {digest} != {want}"));
                    return Err(AbError::VerificationFailed(format!(
                        "回读校验不符: {digest} != {want}"
                    )));
                }
                self.state = TxState::WriteVerified;
                Ok(())
            }
            Err(e) => {
                self.state = TxState::Failed(format!("写槽中断: {e}"));
                Err(AbError::VerificationFailed(format!("写槽中断: {e}")))
            }
        }
    }

    /// 步骤 9：配置 bootloader 试引导非活动槽（bootcount 重置）。
    pub fn arm(&mut self, boot: &mut dyn BootControl, slots: &mut SlotTable) -> AbResult<()> {
        self.require(&[TxState::WriteVerified], "arm")?;
        // 槽内指纹登记（旧镜像完好性验证的数据基础）
        let digest = self
            .manifest
            .as_ref()
            .expect("begin 后必有清单")
            .images
            .first()
            .expect("非空")
            .checksum
            .clone();
        slots.slot_mut(self.to_slot).image_digest = Some(digest);
        slots.arm(self.to_slot);
        boot.set_trying(self.to_slot)?;
        self.state = TxState::Armed;
        Ok(())
    }

    /// 步骤 10–11：引导尝试结果（重启后由宿主上报）。
    /// 引导失败且计数耗尽 → 回退（步骤 13）。
    pub fn record_boot_result(
        &mut self,
        boot: &mut dyn BootControl,
        slots: &mut SlotTable,
        booted_ok: bool,
    ) -> AbResult<()> {
        self.require(&[TxState::Armed], "record_boot_result")?;
        if booted_ok {
            self.state = TxState::HealthChecking;
            Ok(())
        } else {
            let exhausted = slots.record_boot_attempt(self.to_slot).is_err();
            if exhausted {
                self.rollback(boot, slots)?;
                self.state = TxState::RolledBack;
                Err(AbError::Boot(format!(
                    "{:?} 引导计数耗尽——已回退到 {:?}",
                    self.to_slot, self.from_slot
                )))
            } else {
                // 计数内：保持 Armed（bootloader 会再次尝试）
                Ok(())
            }
        }
    }

    /// 步骤 11–13：健康检查裁决。通过 → 槽位确认 + 迁移凭证签发；
    /// 失败 → 回退（旧槽接管，旧镜像完好）。
    pub fn report_health(
        &mut self,
        boot: &mut dyn BootControl,
        slots: &mut SlotTable,
        data: &mut SharedData,
        healthy: bool,
    ) -> AbResult<SlotConfirmation> {
        self.require(&[TxState::HealthChecking], "report_health")?;
        if healthy {
            slots.confirm(self.to_slot);
            boot.mark_good(self.to_slot)?;
            tracing::info!(
                schema = data.schema_version(),
                slot = ?self.to_slot,
                "槽位确认 successful（迁移凭证签发）"
            );
            self.state = TxState::Successful;
            Ok(SlotConfirmation::new(self.to_slot))
        } else {
            self.rollback(boot, slots)?;
            self.state = TxState::RolledBack;
            Err(AbError::Boot(format!(
                "健康检查失败——已回退到 {:?}",
                self.from_slot
            )))
        }
    }

    fn rollback(&mut self, boot: &mut dyn BootControl, slots: &mut SlotTable) -> AbResult<()> {
        slots.rollback_to(self.from_slot)?;
        boot.mark_bad(self.to_slot)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::MockBoot;
    use crate::bundle::{Sha256Verifier, build_manifest};
    use crate::slot::SlotState;

    /// 记录型 writer：成功写/中断注入两态。
    struct MemWriter {
        interrupted: bool,
        written: Option<Vec<u8>>,
    }
    impl MemWriter {
        fn ok() -> Self {
            Self {
                interrupted: false,
                written: None,
            }
        }
        fn interrupted() -> Self {
            Self {
                interrupted: true,
                written: None,
            }
        }
    }
    impl SlotWriter for MemWriter {
        fn write_image(&mut self, image: &[u8]) -> Result<String, String> {
            if self.interrupted {
                return Err("模拟断电：写入中断".into());
            }
            self.written = Some(image.to_vec());
            Ok(loomos_runtime::hash::sha256_hex(image))
        }
    }

    fn image() -> Vec<u8> {
        b"loomos-rootfs-v8".to_vec()
    }

    fn bundle_files() -> Vec<(String, Vec<u8>)> {
        vec![("rootfs.img".into(), image())]
    }

    fn manifest() -> BundleManifest {
        build_manifest(
            "fmql30tai676h",
            bundle_files(),
            BundleManifest {
                version: 1,
                compatible: "fmql30tai676h".into(),
                images: vec![],
                source_commit: Some("abc".into()),
                flake_ref: None,
                wamr_version: None,
            },
        )
    }

    fn new_tx_ok(slots: &SlotTable) -> UpdateTx {
        let mut tx = UpdateTx::new(slots, "fmql30tai676h");
        tx.begin(&Sha256Verifier, manifest(), &bundle_files())
            .unwrap();
        tx
    }

    /// 完整成功路径（§16.4 第 1 项）。
    #[test]
    fn successful_update_full_transaction() {
        let mut slots = SlotTable::initial(SlotId::A, "old", "base-b");
        let mut boot = MockBoot::default();
        let mut data = SharedData::new(1);
        let mut tx = new_tx_ok(&slots);

        tx.write_slot(&mut MemWriter::ok()).unwrap();
        assert_eq!(*tx.state(), TxState::WriteVerified);
        tx.arm(&mut boot, &mut slots).unwrap();
        assert_eq!(slots.slot(SlotId::B).state, SlotState::Trying);
        tx.record_boot_result(&mut boot, &mut slots, true).unwrap();
        assert_eq!(*tx.state(), TxState::HealthChecking);
        let conf = tx
            .report_health(&mut boot, &mut slots, &mut data, true)
            .unwrap();
        assert_eq!(*tx.state(), TxState::Successful);
        assert_eq!(slots.active, SlotId::B);
        assert_eq!(conf.slot(), SlotId::B);
    }

    /// §16.4 第 2 项：写槽中断——槽表不动，旧槽完好，新事务可重启。
    #[test]
    fn interrupted_update_leaves_slots_untouched() {
        let slots = SlotTable::initial(SlotId::A, "old-digest", "old-b");
        let mut tx = new_tx_ok(&slots);
        let err = tx.write_slot(&mut MemWriter::interrupted()).unwrap_err();
        assert!(err.to_string().contains("写槽中断"));
        assert!(matches!(tx.state(), TxState::Failed(_)));
        // 槽表完全未动
        assert_eq!(slots.active, SlotId::A);
        assert_eq!(slots.slot(SlotId::B).state, SlotState::Good);
        assert_eq!(slots.slot(SlotId::B).image_digest.as_deref(), Some("old-b"));
        // 重启 = 新事务重验 bundle 后成功
        let mut tx2 = new_tx_ok(&slots);
        tx2.write_slot(&mut MemWriter::ok()).unwrap();
        assert_eq!(*tx2.state(), TxState::WriteVerified);
    }

    /// §16.4 第 3 项：非法签名/校验和不符——begin 阶段拒绝，槽表不动。
    #[test]
    fn invalid_signature_stage_rejects_before_write() {
        let slots = SlotTable::initial(SlotId::A, "old", "b");
        let mut files = bundle_files();
        // 同长度篡改（专测 checksum 校验路径）
        let mut tampered = files[0].1.clone();
        tampered[0] ^= 0xFF;
        files[0].1 = tampered;
        let mut tx = UpdateTx::new(&slots, "fmql30tai676h");
        let err = tx.begin(&Sha256Verifier, manifest(), &files).unwrap_err();
        assert!(err.to_string().contains("校验和不符"));
        assert!(matches!(tx.state(), TxState::Failed(_)));
        assert_eq!(slots.active, SlotId::A, "校验失败不得触碰槽表");
        // Failed 后再 begin = 非法转换
        assert!(matches!(
            tx.begin(&Sha256Verifier, manifest(), &bundle_files()),
            Err(AbError::InvalidTransition { .. })
        ));
    }

    /// §16.4 第 5 项：板型不匹配拒绝。
    #[test]
    fn wrong_board_rejected() {
        let slots = SlotTable::initial(SlotId::A, "a", "b");
        let mut tx = UpdateTx::new(&slots, "other-board");
        let err = tx
            .begin(&Sha256Verifier, manifest(), &bundle_files())
            .unwrap_err();
        assert!(err.to_string().contains("板型不匹配"));
    }

    /// §16.4 第 6/8 项：新槽引导失败 → 计数耗尽 → 回退，旧槽完好。
    #[test]
    fn boot_failure_exhausts_counter_then_rolls_back() {
        let mut slots = SlotTable::initial(SlotId::A, "old-digest", "old-b");
        let mut boot = MockBoot::failing(SlotId::B);
        let data = SharedData::new(1);
        let mut tx = new_tx_ok(&slots);
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        tx.arm(&mut boot, &mut slots).unwrap();

        // 三次引导尝试全失败：1/3、2/3 计数内保持 Armed，3/3 耗尽回退
        assert!(tx.record_boot_result(&mut boot, &mut slots, false).is_ok());
        assert_eq!(*tx.state(), TxState::Armed);
        assert!(tx.record_boot_result(&mut boot, &mut slots, false).is_ok());
        let err = tx
            .record_boot_result(&mut boot, &mut slots, false)
            .unwrap_err();
        assert!(err.to_string().contains("计数耗尽"));
        assert_eq!(*tx.state(), TxState::RolledBack);
        // 回退语义：active 回旧槽，新槽 Bad
        assert_eq!(slots.active, SlotId::A);
        assert_eq!(slots.slot(SlotId::B).state, SlotState::Bad);
        assert_eq!(
            slots.image_digest(SlotId::A),
            Some("old-digest"),
            "旧镜像完好"
        );
        assert_eq!(boot.current(), SlotId::A);
        let _ = data;
    }

    /// §16.4 第 7 项：健康检查失败 → 回退。
    #[test]
    fn health_check_failure_rolls_back() {
        let mut slots = SlotTable::initial(SlotId::A, "old", "b");
        let mut boot = MockBoot::default();
        let mut data = SharedData::new(1);
        let mut tx = new_tx_ok(&slots);
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        tx.arm(&mut boot, &mut slots).unwrap();
        tx.record_boot_result(&mut boot, &mut slots, true).unwrap();
        let err = tx
            .report_health(&mut boot, &mut slots, &mut data, false)
            .unwrap_err();
        assert!(err.to_string().contains("健康检查失败"));
        assert_eq!(*tx.state(), TxState::RolledBack);
        assert_eq!(slots.active, SlotId::A);
        assert_eq!(slots.slot(SlotId::A).state, SlotState::Good);
    }

    /// §16.4 第 9 项：未确认槽不得执行不可逆迁移（凭证只在成功路径签发）。
    #[test]
    fn irreversible_migration_only_after_confirmation() {
        let mut slots = SlotTable::initial(SlotId::A, "old", "b");
        let mut boot = MockBoot::default();
        let mut data = SharedData::new(1);
        let mut tx = new_tx_ok(&slots);
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        tx.arm(&mut boot, &mut slots).unwrap();
        tx.record_boot_result(&mut boot, &mut slots, true).unwrap();
        // 健康检查失败 → 回退 → 无凭证 → 迁移无从发生（编译期类型闸门 + 运行期验证：
        // 事务返回 Err，没有 SlotConfirmation 可用）
        assert!(
            tx.report_health(&mut boot, &mut slots, &mut data, false)
                .is_err()
        );
        assert_eq!(data.schema_version(), 1, "回滚路径不得触发 schema 迁移");
    }

    /// §16.4 第 10 项：成功路径的健康检查通过后迁移才被授权。
    #[test]
    fn migration_allowed_after_confirmation() {
        let mut slots = SlotTable::initial(SlotId::A, "old", "b");
        let mut boot = MockBoot::default();
        let mut data = SharedData::new(1);
        let mut tx = new_tx_ok(&slots);
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        tx.arm(&mut boot, &mut slots).unwrap();
        tx.record_boot_result(&mut boot, &mut slots, true).unwrap();
        let conf = tx
            .report_health(&mut boot, &mut slots, &mut data, true)
            .unwrap();
        data.apply_irreversible_migration(2, "v2 schema", &conf)
            .unwrap();
        assert_eq!(data.schema_version(), 2);
    }

    /// 非法转换矩阵：跳步全部拒绝。
    #[test]
    fn invalid_transitions_rejected() {
        let mut slots = SlotTable::initial(SlotId::A, "a", "b");
        let mut boot = MockBoot::default();
        let mut data = SharedData::new(1);

        // Idle 直接 write/arm/report
        let mut tx = UpdateTx::new(&slots, "fmql30tai676h");
        assert!(matches!(
            tx.write_slot(&mut MemWriter::ok()),
            Err(AbError::InvalidTransition {
                from: TxState::Idle,
                ..
            })
        ));
        assert!(matches!(
            tx.arm(&mut boot, &mut slots),
            Err(AbError::InvalidTransition {
                from: TxState::Idle,
                ..
            })
        ));
        assert!(matches!(
            tx.report_health(&mut boot, &mut slots, &mut data, true),
            Err(AbError::InvalidTransition {
                from: TxState::Idle,
                ..
            })
        ));

        // BundleVerified 直接 arm（跳过写槽）
        let mut tx = new_tx_ok(&slots);
        assert!(matches!(
            tx.arm(&mut boot, &mut slots),
            Err(AbError::InvalidTransition {
                from: TxState::BundleVerified,
                ..
            })
        ));
        // WriteVerified 直接 report（跳过引导）
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        assert!(matches!(
            tx.report_health(&mut boot, &mut slots, &mut data, true),
            Err(AbError::InvalidTransition {
                from: TxState::WriteVerified,
                ..
            })
        ));
    }

    /// 计数内引导失败保持 Armed（bootloader 重试语义）。
    #[test]
    fn boot_failure_within_counter_stays_armed() {
        let mut slots = SlotTable::initial(SlotId::A, "a", "b");
        let mut boot = MockBoot::failing(SlotId::B);
        let mut tx = new_tx_ok(&slots);
        tx.write_slot(&mut MemWriter::ok()).unwrap();
        tx.arm(&mut boot, &mut slots).unwrap();
        tx.record_boot_result(&mut boot, &mut slots, false).unwrap();
        assert_eq!(*tx.state(), TxState::Armed, "1/3 计数内——保持 Armed 等重试");
        assert_eq!(slots.slot(SlotId::B).boot_attempts, 1);
    }
}
