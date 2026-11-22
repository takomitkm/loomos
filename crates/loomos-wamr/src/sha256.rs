//! SHA-256 正本已上移 `loomos-runtime::hash`（组件包与 A/B bundle 共用）。
//! 本模块保留 re-export 兼容（`loomis-wamr::sha256::sha256_hex`）。

pub use loomos_runtime::hash::Sha256;
pub use loomos_runtime::hash::sha256_hex;
