//! normos 家居域。
//!
//! Home Domain 定义领域操作与策略（architecture/06 §6.5：Device Core 不做万能 API）：
//! - `actions`：意图 → 设备操作映射（旧 executor 的 TARGETS/动作函数等价物）；
//! - `door`：四道闸开门（口令 → 人脸 → PAD 防伪 → 声纹），任一不过不开；
//! - `voiceprint`：声纹录入/校验 mock（192 维余弦 ≥ 阈值 0.5 口径）；
//! - `confirm`：危险动作一次性确认（旧 control/commands.py 契约：绑定动作+参数摘要
//!   +主体+TTL，用过即废）。

pub mod actions;
pub mod confirm;
pub mod door;
pub mod voiceprint;

pub use actions::{ActionOutcome, ActionRegistry};
pub use confirm::{ConfirmationStore, RiskyActionError};
pub use door::{PadMode, UNLOCK_PHRASES, UnlockResult};
pub use voiceprint::MockVoiceprint;
