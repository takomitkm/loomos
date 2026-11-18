//! 实体/区域注册表（对应旧系统 make_registry.py：不给注册表时 NLU 无从解析房间/设备词）。
//!
//! 旧格式是 HA `.storage` 两层 JSON 模拟；新架构直接用结构化 JSON（entity_id 用 ASCII，
//! name/aliases 用中文）。字段映射质量声明（architecture/04 §4.5/纲要 §4.5）：
//! 对 HA entity/area registry 为 `Partial`——只保留 NLU 需要的子集，
//! 不伪装成完整 HA 注册表。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Area {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub name: String,
    pub area_id: String,
    /// light | switch | sensor | camera | speaker | lock
    pub domain: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EntityRegistry {
    pub areas: Vec<Area>,
    pub entities: Vec<Entity>,
}

#[derive(Error, Debug, Clone, PartialEq)]
pub enum RegistryError {
    #[error("找不到区域：{0}")]
    NoSuchArea(String),
    #[error("找不到实体：{0}")]
    NoSuchEntity(String),
    #[error("实体 {0} 的 area {1} 未注册")]
    OrphanEntity(String, String),
    #[error("实体 ID 重复：{0}")]
    DuplicateEntity(String),
}

impl EntityRegistry {
    pub fn load_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn validate(&self) -> Result<(), RegistryError> {
        let area_ids: Vec<&str> = self.areas.iter().map(|a| a.id.as_str()).collect();
        let mut seen = std::collections::BTreeSet::new();
        for e in &self.entities {
            if !area_ids.contains(&e.area_id.as_str()) {
                return Err(RegistryError::OrphanEntity(
                    e.entity_id.clone(),
                    e.area_id.clone(),
                ));
            }
            if !seen.insert(e.entity_id.clone()) {
                return Err(RegistryError::DuplicateEntity(e.entity_id.clone()));
            }
        }
        Ok(())
    }

    /// 找区域：name 或 aliases 精确匹配。
    pub fn find_area(&self, word: &str) -> Option<&Area> {
        self.areas
            .iter()
            .find(|a| a.name == word || a.aliases.iter().any(|s| s == word))
    }

    /// 在区域内找实体：name/domain 中文名 或 aliases 匹配。
    pub fn find_entity_in_area(&self, area_id: &str, word: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| {
            e.area_id == area_id
                && (e.name.contains(word)
                    || e.aliases.iter().any(|s| s == word)
                    || e.domain == word)
        })
    }

    /// 全局找实体（无区域词时）。
    pub fn find_entity_global(&self, word: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| {
            e.name.contains(word) || e.aliases.iter().any(|s| s == word) || e.domain == word
        })
    }

    pub fn entity(&self, entity_id: &str) -> Option<&Entity> {
        self.entities.iter().find(|e| e.entity_id == entity_id)
    }

    /// 导出只读索引（控制台/调试用）。
    pub fn summary(&self) -> BTreeMap<String, String> {
        self.entities
            .iter()
            .map(|e| (e.entity_id.clone(), format!("{}（{}）", e.name, e.area_id)))
            .collect()
    }
}

/// 默认注册表：对齐旧板 3 区域 + 6 实体的语义（客厅灯/卧室灯/门口摄像头/录制开关/门锁/音箱）。
impl EntityRegistry {
    pub fn default_home() -> Self {
        Self {
            areas: vec![
                Area {
                    id: "keting".into(),
                    name: "客厅".into(),
                    aliases: vec!["大厅".into(), "前厅".into()],
                },
                Area {
                    id: "woshi".into(),
                    name: "卧室".into(),
                    aliases: vec!["睡房".into()],
                },
                Area {
                    id: "menkou".into(),
                    name: "门口".into(),
                    aliases: vec!["门外".into(), "大门".into()],
                },
            ],
            entities: vec![
                Entity {
                    entity_id: "light.keting".into(),
                    name: "客厅的灯".into(),
                    area_id: "keting".into(),
                    domain: "light".into(),
                    aliases: vec!["灯".into(), "大灯".into(), "顶灯".into()],
                },
                Entity {
                    entity_id: "light.woshi".into(),
                    name: "卧室的灯".into(),
                    area_id: "woshi".into(),
                    domain: "light".into(),
                    aliases: vec!["灯".into(), "床头灯".into()],
                },
                Entity {
                    entity_id: "camera.menkou".into(),
                    name: "门口摄像头".into(),
                    area_id: "menkou".into(),
                    domain: "camera".into(),
                    aliases: vec!["摄像头".into(), "画面".into()],
                },
                Entity {
                    entity_id: "switch.recorder".into(),
                    name: "自动录制".into(),
                    area_id: "menkou".into(),
                    domain: "switch".into(),
                    aliases: vec!["录制".into(), "录像".into()],
                },
                Entity {
                    entity_id: "lock.door".into(),
                    name: "门锁".into(),
                    area_id: "menkou".into(),
                    domain: "lock".into(),
                    aliases: vec!["门".into(), "锁".into()],
                },
                Entity {
                    entity_id: "speaker.tts".into(),
                    name: "音箱".into(),
                    area_id: "keting".into(),
                    domain: "speaker".into(),
                    aliases: vec!["喇叭".into()],
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_registry_resolves_area_and_entity() {
        let reg = EntityRegistry::default_home();
        reg.validate().unwrap();
        let area = reg.find_area("客厅").unwrap();
        assert_eq!(area.id, "keting");
        let e = reg.find_entity_in_area("keting", "灯").unwrap();
        assert_eq!(e.entity_id, "light.keting");
        // 卧室的"灯"必须是另一盏
        let w = reg.find_entity_in_area("woshi", "灯").unwrap();
        assert_eq!(w.entity_id, "light.woshi");
    }

    #[test]
    fn area_alias_resolves() {
        let reg = EntityRegistry::default_home();
        assert_eq!(reg.find_area("大门").unwrap().id, "menkou");
    }

    #[test]
    fn orphan_entity_rejected() {
        let reg = EntityRegistry {
            areas: vec![],
            entities: vec![Entity {
                entity_id: "light.x".into(),
                name: "灯".into(),
                area_id: "ghost".into(),
                domain: "light".into(),
                aliases: vec![],
            }],
        };
        assert!(matches!(
            reg.validate(),
            Err(RegistryError::OrphanEntity(_, _))
        ));
    }

    #[test]
    fn json_roundtrip() {
        let reg = EntityRegistry::default_home();
        let json = serde_json::to_string_pretty(&reg).unwrap();
        let back = EntityRegistry::load_json(&json).unwrap();
        assert_eq!(back.entities.len(), reg.entities.len());
    }
}
