#![forbid(unsafe_code)]

//! loomos NLU。
//!
//! 双通道继承旧系统 klar_executor 的分流纪律：
//! - **通道 B（直连表）**：整句归一化精确匹配，确定性，~0 开销；
//!   「不要开始录制」「怎么开始录制」**不得**命中（只做整句匹配，不做子串）；
//! - **通道 A（规则语法）**：中文家居词槽位解析（本 crate 默认实现），
//!   可切换为外部 klar 引擎适配器（HTTP /api/v2/parse，decision/candidates 契约）。
//!
//! klar 只做确定性解析、不执行——本 crate 同样只产 Intent，不碰设备。

pub mod grammar;
pub mod intents;
pub mod registry;

pub use grammar::{NluDecision, RuleNlu};
pub use intents::{Intent, IntentName, Slots};
pub use registry::{Area, Entity, EntityRegistry};
