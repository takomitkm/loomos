//! 危险动作一次性确认（旧 control/commands.py `_consume_confirmation` 契约）：
//! 绑定 action + params 摘要 + principal + TTL；用过即废；同键异参拒绝。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;
use thiserror::Error;

pub const DEFAULT_TTL: Duration = Duration::from_secs(120);

#[derive(Error, Debug, Clone, PartialEq)]
pub enum RiskyActionError {
    #[error("没有待确认的 {0}")]
    NoPendingConfirmation(String),
    #[error("确认码不匹配（绑定动作/参数不符）")]
    ConfirmationMismatch,
    #[error("确认已过期（TTL {0:?}）")]
    Expired(Duration),
    #[error("确认主体不符：{0}")]
    PrincipalMismatch(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingConfirmation {
    pub action: String,
    pub params_digest: String,
    pub principal: String,
    pub code: String,
    pub issued_ms: u64,
    pub ttl: Duration,
}

fn params_digest(params: &serde_json::Value) -> String {
    // 规范化 JSON 摘要（旧系统 canonical_json 口径的简化版：serde_json 序列化本身有序）
    let s = serde_json::to_string(params).unwrap_or_default();
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

/// 危险动作确认存储。
pub struct ConfirmationStore {
    pending: BTreeMap<String, PendingConfirmation>,
    now_ms: fn() -> u64,
    next_code: u64,
}

impl Default for ConfirmationStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfirmationStore {
    pub fn new() -> Self {
        Self {
            now_ms: unix_ms,
            next_code: 1000,
            pending: BTreeMap::new(),
        }
    }

    pub fn set_clock(&mut self, f: fn() -> u64) {
        self.now_ms = f;
    }

    /// 为危险动作签发一次性确认。
    pub fn issue(
        &mut self,
        action: &str,
        params: &serde_json::Value,
        principal: &str,
    ) -> PendingConfirmation {
        let code = format!("CONF-{}", self.next_code);
        self.next_code += 1;
        let c = PendingConfirmation {
            action: action.to_string(),
            params_digest: params_digest(params),
            principal: principal.to_string(),
            code: code.clone(),
            issued_ms: (self.now_ms)(),
            ttl: DEFAULT_TTL,
        };
        self.pending.insert(code.clone(), c.clone());
        tracing::info!(action, principal, code=%code, "risky action pending confirmation");
        c
    }

    /// 消费确认：用过即废。绑定（动作+参数+主体+TTL）任何一项不符都拒绝。
    pub fn consume(
        &mut self,
        code: &str,
        action: &str,
        params: &serde_json::Value,
        principal: &str,
    ) -> Result<(), RiskyActionError> {
        let (c_action, c_digest, c_principal, c_issued, c_ttl) = {
            let c = self
                .pending
                .get(code)
                .ok_or_else(|| RiskyActionError::NoPendingConfirmation(action.to_string()))?;
            (
                c.action.clone(),
                c.params_digest.clone(),
                c.principal.clone(),
                c.issued_ms,
                c.ttl,
            )
        };
        if c_action != action || c_digest != params_digest(params) {
            return Err(RiskyActionError::ConfirmationMismatch);
        }
        if c_principal != principal {
            return Err(RiskyActionError::PrincipalMismatch(principal.to_string()));
        }
        let now = (self.now_ms)();
        if now.saturating_sub(c_issued) > c_ttl.as_millis() as u64 {
            self.pending.remove(code);
            return Err(RiskyActionError::Expired(c_ttl));
        }
        self.pending.remove(code);
        tracing::info!(action, principal, code, "risky action confirmed");
        Ok(())
    }

    /// 清理过期确认。
    pub fn sweep(&mut self) {
        let now = (self.now_ms)();
        self.pending
            .retain(|_, c| now.saturating_sub(c.issued_ms) <= c.ttl.as_millis() as u64);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with_fixed_time() -> (ConfirmationStore, fn() -> u64) {
        fn fixed() -> u64 {
            1_000_000
        }
        let mut s = ConfirmationStore::new();
        s.set_clock(fixed);
        (s, fixed)
    }

    #[test]
    fn consume_once_then_gone() {
        let (mut s, _) = store_with_fixed_time();
        let c = s.issue("reboot", &serde_json::json!({"delay_s": 60}), "voice");
        s.consume(
            &c.code,
            "reboot",
            &serde_json::json!({"delay_s": 60}),
            "voice",
        )
        .unwrap();
        // 用过即废
        assert!(matches!(
            s.consume(
                &c.code,
                "reboot",
                &serde_json::json!({"delay_s": 60}),
                "voice"
            ),
            Err(RiskyActionError::NoPendingConfirmation(_))
        ));
    }

    #[test]
    fn params_mismatch_rejected() {
        let (mut s, _) = store_with_fixed_time();
        let c = s.issue("reboot", &serde_json::json!({"delay_s": 60}), "voice");
        assert!(matches!(
            s.consume(
                &c.code,
                "reboot",
                &serde_json::json!({"delay_s": 1}),
                "voice"
            ),
            Err(RiskyActionError::ConfirmationMismatch)
        ));
    }

    #[test]
    fn principal_mismatch_rejected() {
        let (mut s, _) = store_with_fixed_time();
        let c = s.issue("reboot", &serde_json::json!({}), "voice");
        assert!(matches!(
            s.consume(&c.code, "reboot", &serde_json::json!({}), "console"),
            Err(RiskyActionError::PrincipalMismatch(_))
        ));
    }

    #[test]
    fn expiry_enforced() {
        let (mut s, fixed) = store_with_fixed_time();
        let c = s.issue("reboot", &serde_json::json!({}), "voice");
        // 拨快时钟超过 TTL
        fn later() -> u64 {
            1_000_000 + 121_000
        }
        let _ = fixed;
        s.set_clock(later);
        assert!(matches!(
            s.consume(&c.code, "reboot", &serde_json::json!({}), "voice"),
            Err(RiskyActionError::Expired(_))
        ));
    }
}
