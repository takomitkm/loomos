//! Configuration Service（architecture/04）。
//!
//! 定位：跨 Domain 的系统服务。组件通过 ConfigNamespace 访问配置；
//! 本模块统一执行权限、Schema、Revision、事务、事件与 Desired/Applied 协调。
//!
//! 纪律：
//! - Namespace 路径规范化，拒绝 `..`、空段、非法分隔符与路径逃逸（04 §4.3）；
//! - Float 拒绝 NaN/Infinity；Bytes 只用于小型配置数据（04 §4.5）；
//! - 配置写入成功 ≠ 服务已应用；失败保留 desired/applied 差异，不伪造成功（04 §4.9）；
//! - 发布事件必须在事务提交之后；可靠事件由 Outbox 负责（04 §4.8）；
//! - Secret 值在日志/事件中脱敏（04 §4.10）。

use crate::capability::{CapabilityError, CapabilityStore, Rights, Token};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

/// 配置值类型（04 §4.5 第一版）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConfigValue {
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    StringList(Vec<String>),
}

impl ConfigValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Bool(_) => "bool",
            Self::Integer(_) => "integer",
            Self::Float(_) => "float",
            Self::String(_) => "string",
            Self::Bytes(_) => "bytes",
            Self::StringList(_) => "string_list",
        }
    }

    /// Bytes/大资产上限：超过即拒绝（模型/固件/图片走独立 Asset 存储，04 §4.5）。
    pub const MAX_BYTES_LEN: usize = 16 * 1024;

    fn validate(&self) -> Result<(), ConfigError> {
        match self {
            Self::Float(f) if !f.is_finite() => Err(ConfigError::ValidationFailed(
                "Float 拒绝 NaN/Infinity".into(),
            )),
            Self::Bytes(b) if b.len() > Self::MAX_BYTES_LEN => {
                Err(ConfigError::ResourceExhausted(format!(
                    "Bytes 超过 {} 上限，应使用独立 Asset 存储",
                    Self::MAX_BYTES_LEN
                )))
            }
            _ => Ok(()),
        }
    }
}

impl fmt::Display for ConfigValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bool(b) => write!(f, "{b}"),
            Self::Integer(i) => write!(f, "{i}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::String(s) => write!(f, "{s:?}"),
            Self::Bytes(b) => write!(f, "<bytes:{}>", b.len()),
            Self::StringList(l) => write!(f, "[{}]", l.join(", ")),
        }
    }
}

/// 带版本号的配置值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VersionedConfigValue {
    pub value: ConfigValue,
    pub revision: u64,
}

/// Namespace 路径：规范化并拒绝路径逃逸。
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NamespacePath(String);

impl NamespacePath {
    pub fn new(path: &str) -> Result<Self, ConfigError> {
        if path.is_empty() {
            return Err(ConfigError::ValidationFailed("namespace 不能为空".into()));
        }
        if path.starts_with('/') || path.ends_with('/') {
            return Err(ConfigError::ValidationFailed(format!(
                "namespace 不允许前导/尾部分隔符: {path:?}"
            )));
        }
        for seg in path.split('/') {
            if seg.is_empty() {
                return Err(ConfigError::ValidationFailed(format!(
                    "namespace 不允许空段: {path:?}"
                )));
            }
            if seg == "." || seg == ".." {
                return Err(ConfigError::ValidationFailed(format!(
                    "namespace 不允许路径逃逸: {path:?}"
                )));
            }
            if seg
                .chars()
                .any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            {
                return Err(ConfigError::ValidationFailed(format!(
                    "namespace 段只允许字母/数字/-/_: {path:?}"
                )));
            }
        }
        Ok(Self(path.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_within(&self, parent: &NamespacePath) -> bool {
        self.0 == parent.0 || self.0.starts_with(&format!("{}/", parent.0))
    }
}

impl Default for NamespacePath {
    fn default() -> Self {
        // 合法默认路径（Schema 装配前的事务占位）
        Self("system".to_string())
    }
}

impl TryFrom<String> for NamespacePath {
    type Error = ConfigError;
    fn try_from(s: String) -> Result<Self, ConfigError> {
        Self::new(&s)
    }
}

impl From<NamespacePath> for String {
    fn from(p: NamespacePath) -> String {
        p.0
    }
}

impl fmt::Display for NamespacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 字段 Schema 定义（04 §4.6）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SchemaField {
    pub key: String,
    pub value_type: String, // "bool" | "integer" | "float" | "string" | "bytes" | "string_list"
    pub required: bool,
    pub default: Option<ConfigValue>,
    /// integer 的闭区间约束（演示范围校验；更复杂约束随 Schema 演进）。
    pub min: Option<i64>,
    pub max: Option<i64>,
}

impl SchemaField {
    pub fn validate(&self, value: &ConfigValue) -> Result<(), ConfigError> {
        if value.type_name() != self.value_type {
            return Err(ConfigError::TypeMismatch {
                key: self.key.clone(),
                expected: self.value_type.clone(),
                got: value.type_name().to_string(),
            });
        }
        if let (ConfigValue::Integer(i), Some(min)) = (value, self.min)
            && *i < min
        {
            return Err(ConfigError::ValidationFailed(format!(
                "{} = {i} 小于下界 {min}",
                self.key
            )));
        }
        if let (ConfigValue::Integer(i), Some(max)) = (value, self.max)
            && *i > max
        {
            return Err(ConfigError::ValidationFailed(format!(
                "{} = {i} 超过上界 {max}",
                self.key
            )));
        }
        value.validate()?;
        Ok(())
    }
}

/// 一个 namespace 的 Schema。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Schema {
    pub namespace: String,
    pub schema_version: u32,
    pub fields: Vec<SchemaField>,
}

/// Secret 句柄（04 §4.10）：包住敏感字符串，Drop 时清零内存。
///
/// 纪律：secrecy/zeroize 类 crate 只提供类型辅助，**不等于安全存储**；
/// 这里是纯 Rust 的最小实现——Display/Debug 永远脱敏，Drop 覆写。
#[derive(Clone)]
pub struct SecretHandle {
    inner: Vec<u8>,
}

impl SecretHandle {
    pub fn new(value: &str) -> Self {
        Self {
            inner: value.as_bytes().to_vec(),
        }
    }

    /// 显式取出明文（仅授权代码路径调用；打印/日志永远走 Display）。
    pub fn expose(&self) -> String {
        String::from_utf8_lossy(&self.inner).to_string()
    }

    /// 覆写清零（Drop 自动调用；需要提前清除时手动调用）。
    pub fn wipe(&mut self) {
        for b in self.inner.iter_mut() {
            *b = 0;
        }
        self.inner.clear();
    }
}

impl Drop for SecretHandle {
    fn drop(&mut self) {
        self.wipe();
    }
}

impl std::fmt::Display for SecretHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 永不泄明文：只暴露长度
        write!(f, "<secret:len={}>", self.inner.len())
    }
}

impl std::fmt::Debug for SecretHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self, f)
    }
}

/// Schema 迁移注册项（04 §4.6：schema_version + 迁移入口）。
///
/// 语义：namespace 当前 schema_version == from_version 时，可调用
/// `migrate_namespace` 升级到 to_version；迁移函数收到旧数据（key → value），
/// 返回新写入/删除集合与**新 Schema**；单事务原子提交。
/// 迁移函数类型：旧数据 → 产出 writes/deletes/new_schema。
pub type MigrationFn =
    std::sync::Arc<dyn Fn(&BTreeMap<String, ConfigValue>) -> MigrationOutput + Send + Sync>;

pub struct SchemaMigrationSpec {
    pub namespace: String,
    pub from_version: u32,
    pub to_version: u32,
    pub migrate: MigrationFn,
}

#[derive(Debug, Clone, Default)]
pub struct MigrationOutput {
    pub writes: Vec<(String, ConfigValue)>,
    pub deletes: Vec<String>,
    pub new_schema: Schema,
}

/// Provider 只负责数据存取（04 §4.7）。
pub trait ConfigProvider: Send + Sync {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<VersionedConfigValue>, ConfigError>;
    fn list(&self, namespace: &str) -> Result<Vec<String>, ConfigError>;
    /// 原子提交一批写入并返回新 revision。
    fn commit(&self, tx: &ConfigTransaction) -> Result<u64, ConfigError>;
}

/// 配置事务（04 §4.8）：validate → check expected_revision → write → bump revision。
#[derive(Debug, Clone, Default)]
pub struct ConfigTransaction {
    pub namespace: NamespacePath,
    pub expected_revision: u64,
    pub writes: Vec<(String, ConfigValue)>,
    pub deletes: Vec<String>,
}

/// 内存 Provider：单线程内使用，跨线程需外部加锁。
#[derive(Debug, Default)]
pub struct MemoryProvider {
    // namespace -> key -> (value, revision)
    store: std::sync::Mutex<BTreeMap<String, BTreeMap<String, VersionedConfigValue>>>,
    revisions: std::sync::Mutex<BTreeMap<String, u64>>,
}

impl MemoryProvider {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ConfigProvider for MemoryProvider {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<VersionedConfigValue>, ConfigError> {
        let store = self.store.lock().unwrap();
        let rev = self.revisions.lock().unwrap();
        Ok(store
            .get(namespace)
            .and_then(|m| m.get(key))
            .cloned()
            .map(|mut v| {
                v.revision = rev.get(namespace).copied().unwrap_or(0);
                v
            }))
    }

    fn list(&self, namespace: &str) -> Result<Vec<String>, ConfigError> {
        let store = self.store.lock().unwrap();
        Ok(store
            .get(namespace)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn commit(&self, tx: &ConfigTransaction) -> Result<u64, ConfigError> {
        let mut store = self.store.lock().unwrap();
        let mut revisions = self.revisions.lock().unwrap();
        let current = revisions.get(tx.namespace.as_str()).copied().unwrap_or(0);
        if tx.expected_revision != current {
            return Err(ConfigError::RevisionConflict {
                namespace: tx.namespace.to_string(),
                expected: tx.expected_revision,
                actual: current,
            });
        }
        for (_key, value) in &tx.writes {
            value.validate()?;
        }
        let ns_entry = store.entry(tx.namespace.to_string()).or_default();
        for (key, value) in &tx.writes {
            ns_entry.insert(
                key.clone(),
                VersionedConfigValue {
                    value: value.clone(),
                    revision: 0,
                },
            );
        }
        for key in &tx.deletes {
            ns_entry.remove(key);
        }
        let new_rev = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        revisions.insert(tx.namespace.to_string(), new_rev);
        Ok(new_rev)
    }
}

/// JSON 文件 Provider：原子写（temp + rename）。原型阶段的主持久化 Provider。
#[derive(Debug)]
pub struct JsonFileProvider {
    path: PathBuf,
    inner: Mutex<BTreeMap<String, BTreeMap<String, VersionedConfigValue>>>,
    revisions: Mutex<BTreeMap<String, u64>>,
}

#[derive(Serialize, Deserialize, Default)]
struct FileShape {
    namespaces: BTreeMap<String, BTreeMap<String, ConfigValue>>,
    revisions: BTreeMap<String, u64>,
}

impl JsonFileProvider {
    pub fn open(path: PathBuf) -> Result<Self, ConfigError> {
        let inner = if path.exists() {
            let raw = std::fs::read_to_string(&path).map_err(|e| {
                ConfigError::ProviderUnavailable(format!("读取 {}: {e}", path.display()))
            })?;
            let parsed: FileShape = serde_json::from_str(&raw).map_err(|e| {
                ConfigError::SerializationError(format!("解析 {}: {e}", path.display()))
            })?;
            let store = parsed
                .namespaces
                .into_iter()
                .map(|(ns, m)| {
                    (
                        ns,
                        m.into_iter()
                            .map(|(k, v)| {
                                (
                                    k,
                                    VersionedConfigValue {
                                        value: v,
                                        revision: 0,
                                    },
                                )
                            })
                            .collect(),
                    )
                })
                .collect();
            (store, parsed.revisions)
        } else {
            (BTreeMap::new(), BTreeMap::new())
        };
        Ok(Self {
            path,
            inner: Mutex::new(inner.0),
            revisions: Mutex::new(inner.1),
        })
    }

    fn persist(
        &self,
        store: &BTreeMap<String, BTreeMap<String, VersionedConfigValue>>,
        revisions: &BTreeMap<String, u64>,
    ) -> Result<(), ConfigError> {
        let shape = FileShape {
            namespaces: store
                .iter()
                .map(|(ns, m)| {
                    (
                        ns.clone(),
                        m.iter()
                            .map(|(k, v)| (k.clone(), v.value.clone()))
                            .collect(),
                    )
                })
                .collect(),
            revisions: revisions.clone(),
        };
        let tmp = self.path.with_extension("tmp");
        let body = serde_json::to_string_pretty(&shape)
            .map_err(|e| ConfigError::SerializationError(e.to_string()))?;
        std::fs::write(&tmp, body).map_err(|e| {
            ConfigError::ProviderUnavailable(format!("写入 {}: {e}", tmp.display()))
        })?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            ConfigError::ProviderUnavailable(format!("原子替换 {}: {e}", self.path.display()))
        })?;
        Ok(())
    }
}

impl ConfigProvider for JsonFileProvider {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<VersionedConfigValue>, ConfigError> {
        let store = self.inner.lock().unwrap();
        let rev = self.revisions.lock().unwrap();
        Ok(store
            .get(namespace)
            .and_then(|m| m.get(key))
            .cloned()
            .map(|mut v| {
                v.revision = rev.get(namespace).copied().unwrap_or(0);
                v
            }))
    }

    fn list(&self, namespace: &str) -> Result<Vec<String>, ConfigError> {
        let store = self.inner.lock().unwrap();
        Ok(store
            .get(namespace)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default())
    }

    fn commit(&self, tx: &ConfigTransaction) -> Result<u64, ConfigError> {
        let mut store = self.inner.lock().unwrap();
        let mut revisions = self.revisions.lock().unwrap();
        let current = revisions.get(tx.namespace.as_str()).copied().unwrap_or(0);
        if tx.expected_revision != current {
            return Err(ConfigError::RevisionConflict {
                namespace: tx.namespace.to_string(),
                expected: tx.expected_revision,
                actual: current,
            });
        }
        for (_, value) in &tx.writes {
            value.validate()?;
        }
        let ns_entry = store.entry(tx.namespace.to_string()).or_default();
        for (key, value) in &tx.writes {
            ns_entry.insert(
                key.clone(),
                VersionedConfigValue {
                    value: value.clone(),
                    revision: 0,
                },
            );
        }
        for key in &tx.deletes {
            ns_entry.remove(key);
        }
        let new_rev = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        revisions.insert(tx.namespace.to_string(), new_rev);
        self.persist(&store, &revisions)?;
        Ok(new_rev)
    }
}

/// Outbox 事件：事务提交后才发布（04 §4.8）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigChanged {
    pub seq: u64,
    pub namespace: String,
    pub revision: u64,
    pub keys: Vec<String>,
}

/// 配置错误全集（04 §4.8）。
#[derive(Error, Debug, Clone, PartialEq)]
pub enum ConfigError {
    #[error("namespace {0} 不存在")]
    NamespaceNotFound(String),
    #[error("key {0} 不存在")]
    KeyNotFound(String),
    #[error("{key} 类型不符：期望 {expected}，实际 {got}")]
    TypeMismatch {
        key: String,
        expected: String,
        got: String,
    },
    #[error("权限不足：{0}")]
    PermissionDenied(String),
    #[error("校验失败：{0}")]
    ValidationFailed(String),
    #[error("revision 冲突：namespace {namespace} 期望 {expected}，实际 {actual}")]
    RevisionConflict {
        namespace: String,
        expected: u64,
        actual: u64,
    },
    #[error("provider 不可用：{0}")]
    ProviderUnavailable(String),
    #[error("序列化失败：{0}")]
    SerializationError(String),
    #[error("资源耗尽：{0}")]
    ResourceExhausted(String),
}

impl From<CapabilityError> for ConfigError {
    fn from(e: CapabilityError) -> Self {
        ConfigError::PermissionDenied(e.to_string())
    }
}

/// Desired/Applied/Observed 记录（04 §4.9）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyStatus {
    pub namespace: String,
    pub desired_revision: u64,
    pub applied_revision: Option<u64>,
    pub observed_revision: Option<u64>,
    pub last_error: Option<String>,
}

/// Configuration Service：授权 + Schema + Revision + 事件 + Desired/Applied。
pub struct ConfigService {
    provider: Box<dyn ConfigProvider>,
    capabilities: std::sync::Arc<CapabilityStore>,
    schemas: Mutex<BTreeMap<String, Schema>>,
    outbox: Mutex<Vec<ConfigChanged>>,
    outbox_seq: AtomicU64,
    apply_status: Mutex<BTreeMap<String, ApplyStatus>>,
    /// Secret key 黑名单：这些 key 的值永不进入日志/事件。
    secret_keys: Vec<String>,
    /// Schema 迁移注册表：(namespace, from_version) → 迁移规格。
    migrations: Mutex<BTreeMap<(String, u32), std::sync::Arc<SchemaMigrationSpec>>>,
}

impl ConfigService {
    pub fn new(
        provider: Box<dyn ConfigProvider>,
        capabilities: std::sync::Arc<CapabilityStore>,
    ) -> Self {
        Self {
            provider,
            capabilities,
            schemas: Mutex::new(BTreeMap::new()),
            outbox: Mutex::new(Vec::new()),
            outbox_seq: AtomicU64::new(1),
            apply_status: Mutex::new(BTreeMap::new()),
            secret_keys: vec!["token".into(), "secret".into(), "password".into()],
            migrations: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn capabilities(&self) -> &std::sync::Arc<CapabilityStore> {
        &self.capabilities
    }

    /// 注册 Schema（组件声明自己的 Schema 和默认值）。
    pub fn register_schema(&self, schema: Schema) -> Result<(), ConfigError> {
        for f in &schema.fields {
            if !matches!(
                f.value_type.as_str(),
                "bool" | "integer" | "float" | "string" | "bytes" | "string_list"
            ) {
                return Err(ConfigError::ValidationFailed(format!(
                    "字段 {} 的 value_type {} 不受支持",
                    f.key, f.value_type
                )));
            }
        }
        self.schemas
            .lock()
            .unwrap()
            .insert(schema.namespace.clone(), schema);
        Ok(())
    }

    fn schema_of(&self, namespace: &str) -> Option<Schema> {
        self.schemas.lock().unwrap().get(namespace).cloned()
    }

    /// 读取（typed get + 能力校验）。
    pub fn get(
        &self,
        token: &Token,
        namespace: &NamespacePath,
        key: &str,
    ) -> Result<VersionedConfigValue, ConfigError> {
        self.capabilities.check(
            token,
            &Rights::ConfigRead {
                namespace: namespace.to_string(),
            },
        )?;
        self.provider
            .get(namespace.as_str(), key)?
            .ok_or_else(|| ConfigError::KeyNotFound(key.to_string()))
    }

    /// 事务提交：validate → expected_revision → commit → outbox（提交后发布）。
    pub fn commit(&self, token: &Token, tx: ConfigTransaction) -> Result<u64, ConfigError> {
        self.capabilities.check(
            token,
            &Rights::ConfigWrite {
                namespace: tx.namespace.to_string(),
            },
        )?;
        // Schema 校验
        if let Some(schema) = self.schema_of(tx.namespace.as_str()) {
            for (key, value) in &tx.writes {
                if let Some(field) = schema.fields.iter().find(|f| &f.key == key) {
                    field.validate(value)?;
                } else {
                    return Err(ConfigError::ValidationFailed(format!(
                        "{} 不是 {} 的已知字段（Schema 拒绝未知字段）",
                        key, tx.namespace
                    )));
                }
            }
            for f in &schema.fields {
                if f.required && !tx.writes.iter().any(|(k, _)| k == &f.key) {
                    let exists = self.provider.get(tx.namespace.as_str(), &f.key)?.is_some();
                    if !exists && tx.deletes.iter().any(|d| d == &f.key) {
                        return Err(ConfigError::ValidationFailed(format!(
                            "必填字段 {} 不可删除",
                            f.key
                        )));
                    }
                }
            }
        }
        // Secret 不进入 outbox/日志
        let keys: Vec<String> = tx
            .writes
            .iter()
            .map(|(k, _)| {
                if self.is_secret_key(k) {
                    format!("{k}=<redacted>")
                } else {
                    k.clone()
                }
            })
            .collect();
        let new_rev = self.provider.commit(&tx)?;
        // 提交成功后才发布事件（Outbox 语义）
        let event = ConfigChanged {
            seq: self.outbox_seq.fetch_add(1, Ordering::Relaxed),
            namespace: tx.namespace.to_string(),
            revision: new_rev,
            keys,
        };
        self.outbox.lock().unwrap().push(event.clone());
        tracing::info!(ns=%event.namespace, rev=%event.revision, "config committed");
        Ok(new_rev)
    }

    fn is_secret_key(&self, key: &str) -> bool {
        self.secret_keys
            .iter()
            .any(|s| key.to_lowercase().contains(s))
    }

    /// 取出并清空 outbox（持久化由 EventLog 承接）。
    pub fn drain_outbox(&self) -> Vec<ConfigChanged> {
        std::mem::take(&mut *self.outbox.lock().unwrap())
    }

    /// 注册 Schema 迁移规格（from_version → to_version）。
    pub fn register_migration(&self, spec: SchemaMigrationSpec) {
        self.migrations.lock().unwrap().insert(
            (spec.namespace.clone(), spec.from_version),
            std::sync::Arc::new(spec),
        );
    }

    /// 执行 namespace 的 Schema 迁移（04 §4.6 迁移入口）：
    /// 读当前数据 → 迁移函数产出 writes/deletes/new_schema → 单事务原子提交。
    /// 校验：token 需有该 namespace 的 ConfigWrite；当前 schema_version 必须 == from_version。
    pub fn migrate_namespace(&self, token: &Token, namespace: &str) -> Result<u64, ConfigError> {
        self.capabilities.check(
            token,
            &Rights::ConfigWrite {
                namespace: namespace.to_string(),
            },
        )?;
        // 当前 schema 与版本
        let current = self
            .schema_of(namespace)
            .ok_or_else(|| ConfigError::NamespaceNotFound(namespace.to_string()))?;
        let spec = {
            let reg = self.migrations.lock().unwrap();
            reg.get(&(namespace.to_string(), current.schema_version))
                .cloned()
                .ok_or_else(|| {
                    ConfigError::ValidationFailed(format!(
                        "{namespace} 无 schema_version {} 的迁移注册",
                        current.schema_version
                    ))
                })?
        };
        // 读当前数据（Provider list + get）
        let mut old = BTreeMap::new();
        for key in self.provider.list(namespace)? {
            if let Some(v) = self.provider.get(namespace, &key)? {
                old.insert(key, v.value);
            }
        }
        let out = (spec.migrate)(&old);
        // 校验迁移产出的新 Schema 合法（复用 register_schema 校验）
        let mut new_schema = out.new_schema;
        new_schema.namespace = namespace.to_string();
        if new_schema.schema_version != spec.to_version {
            return Err(ConfigError::ValidationFailed(format!(
                "迁移产出 schema_version {} 与 to_version {} 不符",
                new_schema.schema_version, spec.to_version
            )));
        }
        self.register_schema(new_schema)?;
        // 单事务提交
        let tx = ConfigTransaction {
            namespace: NamespacePath::new(namespace)?,
            expected_revision: self.provider_revision(namespace)?,
            writes: out.writes,
            deletes: out.deletes,
        };
        let new_rev = self.provider.commit(&tx)?;
        self.outbox.lock().unwrap().push(ConfigChanged {
            seq: self.outbox_seq.fetch_add(1, Ordering::Relaxed),
            namespace: namespace.to_string(),
            revision: new_rev,
            keys: vec![format!(
                "<schema migration v{}→v{}>",
                spec.from_version, spec.to_version
            )],
        });
        tracing::info!(ns=%namespace, from=spec.from_version, to=spec.to_version, rev=%new_rev, "schema migrated");
        Ok(new_rev)
    }

    /// 读 namespace 当前 revision（provider 各实现把版本挂在 revisions 表）。
    fn provider_revision(&self, namespace: &str) -> Result<u64, ConfigError> {
        // provider.list 后任一 key 的 VersionedConfigValue.revision 即当前 revision
        for key in self.provider.list(namespace)? {
            if let Some(v) = self.provider.get(namespace, &key)? {
                return Ok(v.revision);
            }
        }
        Ok(0)
    }

    /// 读取 Secret 值（04 §4.10）：仅 secret key 可走此入口，返回 Drop 清零的句柄。
    /// 非 secret key 拒绝（防止误把普通配置当敏感值处理，或反之绕过脱敏）。
    pub fn get_secret(
        &self,
        token: &Token,
        namespace: &NamespacePath,
        key: &str,
    ) -> Result<SecretHandle, ConfigError> {
        if !self.is_secret_key(key) {
            return Err(ConfigError::ValidationFailed(format!(
                "{key} 不是 secret key，请走 get()（secret 键黑名单: {:?}）",
                self.secret_keys
            )));
        }
        let v = self.get(token, namespace, key)?;
        match &v.value {
            ConfigValue::String(s) => Ok(SecretHandle::new(s)),
            other => Err(ConfigError::TypeMismatch {
                key: key.to_string(),
                expected: "string".into(),
                got: other.type_name().to_string(),
            }),
        }
    }

    /// 登记 Desired；由 Applier 在目标服务应用成功后写 Applied；服务回报 Observed。
    pub fn set_desired(&self, namespace: &str, revision: u64) {
        let mut map = self.apply_status.lock().unwrap();
        let entry = map
            .entry(namespace.to_string())
            .or_insert_with(|| ApplyStatus {
                namespace: namespace.to_string(),
                desired_revision: revision,
                applied_revision: None,
                observed_revision: None,
                last_error: None,
            });
        entry.desired_revision = revision;
        // 新 desired 出现时旧的 applied 不再代表当前目标
        if entry.applied_revision.is_some() && entry.applied_revision != Some(revision) {
            entry.applied_revision = None;
        }
    }

    pub fn set_applied(&self, namespace: &str, revision: u64) {
        let mut map = self.apply_status.lock().unwrap();
        if let Some(st) = map.get_mut(namespace)
            && st.desired_revision == revision
        {
            st.applied_revision = Some(revision);
            st.last_error = None;
        }
    }

    pub fn set_apply_failed(&self, namespace: &str, revision: u64, err: &str) {
        let mut map = self.apply_status.lock().unwrap();
        if let Some(st) = map.get_mut(namespace)
            && st.desired_revision == revision
        {
            st.last_error = Some(err.to_string());
        }
    }

    pub fn apply_status(&self, namespace: &str) -> Option<ApplyStatus> {
        self.apply_status.lock().unwrap().get(namespace).cloned()
    }

    pub fn list_namespaces(&self) -> Vec<String> {
        // provider 侧只暴露 list(keys)，namespace 清单从 revisions 不可取时退化
        self.schemas.lock().unwrap().keys().cloned().collect()
    }

    pub fn provider_list(&self, namespace: &str) -> Result<Vec<String>, ConfigError> {
        self.provider.list(namespace)
    }

    /// 测试/装配辅助：直接访问 provider（绕过授权，仅装配层使用）。
    pub fn provider_get(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<VersionedConfigValue>, ConfigError> {
        self.provider.get(namespace, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityStore;

    fn svc_with_caps() -> (ConfigService, Token, Token) {
        let mut caps = CapabilityStore::new();
        let writer = caps
            .grant(
                "voice",
                Rights::ConfigWrite {
                    namespace: "services/voice".into(),
                },
            )
            .unwrap();
        let reader = caps
            .grant(
                "monitor",
                Rights::ConfigRead {
                    namespace: "services/voice".into(),
                },
            )
            .unwrap();
        let svc = ConfigService::new(Box::new(MemoryProvider::new()), std::sync::Arc::new(caps));
        (svc, writer, reader)
    }

    #[test]
    fn commit_then_get_with_schema() {
        let (svc, writer, reader) = svc_with_caps();
        svc.register_schema(Schema {
            namespace: "services/voice/asr".into(),
            schema_version: 1,
            fields: vec![
                SchemaField {
                    key: "rtf_max".into(),
                    value_type: "integer".into(),
                    required: true,
                    default: Some(ConfigValue::Integer(1)),
                    min: Some(1),
                    max: Some(2),
                },
                SchemaField {
                    key: "model".into(),
                    value_type: "string".into(),
                    required: true,
                    default: Some(ConfigValue::String("zipformer-small-int8".into())),
                    min: None,
                    max: None,
                },
            ],
        })
        .unwrap();
        let ns = NamespacePath::new("services/voice/asr").unwrap();
        let tx = ConfigTransaction {
            namespace: ns.clone(),
            expected_revision: 0,
            writes: vec![
                ("rtf_max".into(), ConfigValue::Integer(1)),
                (
                    "model".into(),
                    ConfigValue::String("zipformer-small-int8".into()),
                ),
            ],
            deletes: vec![],
        };
        let rev = svc.commit(&writer, tx).unwrap();
        let v = svc.get(&reader, &ns, "rtf_max").unwrap();
        assert_eq!(v.value, ConfigValue::Integer(1));
        assert_eq!(v.revision, rev);
        // 未知字段拒绝
        let bad = ConfigTransaction {
            namespace: ns.clone(),
            expected_revision: rev,
            writes: vec![("unknown_key".into(), ConfigValue::Integer(5))],
            deletes: vec![],
        };
        assert!(matches!(
            svc.commit(&writer, bad),
            Err(ConfigError::ValidationFailed(_))
        ));
        // 范围越界拒绝
        let oob = ConfigTransaction {
            namespace: ns,
            expected_revision: rev,
            writes: vec![("rtf_max".into(), ConfigValue::Integer(9))],
            deletes: vec![],
        };
        assert!(matches!(
            svc.commit(&writer, oob),
            Err(ConfigError::ValidationFailed(_))
        ));
    }

    #[test]
    fn revision_conflict_detected() {
        let (svc, writer, _r) = svc_with_caps();
        let ns = NamespacePath::new("services/voice/rt").unwrap();
        let tx = ConfigTransaction {
            namespace: ns.clone(),
            expected_revision: 0,
            writes: vec![("lang".into(), ConfigValue::String("zh-CN".into()))],
            deletes: vec![],
        };
        svc.commit(&writer, tx).unwrap();
        let stale = ConfigTransaction {
            namespace: ns,
            expected_revision: 0,
            writes: vec![("lang".into(), ConfigValue::String("en".into()))],
            deletes: vec![],
        };
        assert!(matches!(
            svc.commit(&writer, stale),
            Err(ConfigError::RevisionConflict { .. })
        ));
    }

    #[test]
    fn outbox_publishes_only_after_commit_and_redacts_secrets() {
        let mut caps = CapabilityStore::new();
        let writer = caps
            .grant(
                "voice",
                Rights::ConfigWrite {
                    namespace: "services/tts".into(),
                },
            )
            .unwrap();
        let svc = ConfigService::new(Box::new(MemoryProvider::new()), std::sync::Arc::new(caps));
        assert!(svc.drain_outbox().is_empty());
        let ns = NamespacePath::new("services/tts").unwrap();
        let tx = ConfigTransaction {
            namespace: ns.clone(),
            expected_revision: 0,
            writes: vec![
                ("volume".into(), ConfigValue::Integer(80)),
                (
                    "api_token".into(),
                    ConfigValue::String("super-secret".into()),
                ),
            ],
            deletes: vec![],
        };
        let rev = svc.commit(&writer, tx).unwrap();
        let outbox = svc.drain_outbox();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox[0].revision, rev);
        // secret 脱敏：key 名仍可见，但事件不携带值（payload 只有 keys）
        assert!(outbox[0].keys.iter().any(|k| k.contains("<redacted>")));
    }

    #[test]
    fn float_nan_rejected() {
        let (svc, writer, _r) = svc_with_caps();
        let ns = NamespacePath::new("services/voice/vad").unwrap();
        let tx = ConfigTransaction {
            namespace: ns,
            expected_revision: 0,
            writes: vec![("threshold".into(), ConfigValue::Float(f64::NAN))],
            deletes: vec![],
        };
        assert!(matches!(
            svc.commit(&writer, tx),
            Err(ConfigError::ValidationFailed(_))
        ));
    }

    #[test]
    fn bytes_over_limit_rejected() {
        let (svc, writer, _r) = svc_with_caps();
        let ns = NamespacePath::new("services/voice/kws").unwrap();
        let tx = ConfigTransaction {
            namespace: ns,
            expected_revision: 0,
            writes: vec![(
                "blob".into(),
                ConfigValue::Bytes(vec![0u8; ConfigValue::MAX_BYTES_LEN + 1]),
            )],
            deletes: vec![],
        };
        assert!(matches!(
            svc.commit(&writer, tx),
            Err(ConfigError::ResourceExhausted(_))
        ));
    }

    #[test]
    fn desired_applied_flow() {
        let (svc, _w, _r) = svc_with_caps();
        svc.set_desired("services/voice", 7);
        let st = svc.apply_status("services/voice").unwrap();
        assert_eq!(st.desired_revision, 7);
        assert_eq!(st.applied_revision, None);
        // 应用失败 → 保留差异与原因，不伪造成功
        svc.set_apply_failed("services/voice", 7, "asr model load timeout");
        let st = svc.apply_status("services/voice").unwrap();
        assert_eq!(st.applied_revision, None);
        assert!(st.last_error.is_some());
        // 应用成功 → 对齐
        svc.set_applied("services/voice", 7);
        let st = svc.apply_status("services/voice").unwrap();
        assert_eq!(st.applied_revision, Some(7));
        assert!(st.last_error.is_none());
    }

    #[test]
    fn secret_handle_redacts_and_wipes() {
        let mut h = SecretHandle::new("super-token");
        // Display/Debug 永不泄明文
        assert!(!format!("{h}").contains("super-token"));
        assert!(!format!("{h:?}").contains("super-token"));
        assert_eq!(format!("{h}"), "<secret:len=11>");
        // expose 只给授权路径
        assert_eq!(h.expose(), "super-token");
        // wipe 覆写清零
        h.wipe();
        assert!(h.expose().is_empty());
    }

    #[test]
    fn get_secret_rejects_non_secret_keys() {
        let (svc, writer, reader) = svc_with_caps();
        let ns = NamespacePath::new("services/voice").unwrap();
        let tx = ConfigTransaction {
            namespace: ns.clone(),
            expected_revision: 0,
            writes: vec![
                ("model".into(), ConfigValue::String("zipformer".into())),
                ("api_token".into(), ConfigValue::String("abc123".into())),
            ],
            deletes: vec![],
        };
        svc.commit(&writer, tx).unwrap();
        // secret key 走 get_secret 成功
        let h = svc.get_secret(&reader, &ns, "api_token").unwrap();
        assert_eq!(h.expose(), "abc123");
        // 非 secret key 拒绝（防绕过脱敏）
        let err = svc.get_secret(&reader, &ns, "model").unwrap_err();
        assert!(matches!(err, ConfigError::ValidationFailed(_)));
    }

    #[test]
    fn schema_migration_flow() {
        let (svc, writer, reader) = svc_with_caps();
        // v1：旧字段名
        svc.register_schema(Schema {
            namespace: "services/voice/tts".into(),
            schema_version: 1,
            fields: vec![SchemaField {
                key: "volume".into(),
                value_type: "integer".into(),
                required: true,
                default: Some(ConfigValue::Integer(50)),
                min: Some(0),
                max: Some(100),
            }],
        })
        .unwrap();
        let ns = NamespacePath::new("services/voice/tts").unwrap();
        svc.commit(
            &writer,
            ConfigTransaction {
                namespace: ns.clone(),
                expected_revision: 0,
                writes: vec![("volume".into(), ConfigValue::Integer(70))],
                deletes: vec![],
            },
        )
        .unwrap();
        // 注册 v1→v2 迁移：volume → gain（映射值域 0-100 → 0-10）
        svc.register_migration(SchemaMigrationSpec {
            namespace: "services/voice/tts".into(),
            from_version: 1,
            to_version: 2,
            migrate: std::sync::Arc::new(|old| {
                let gain = match old.get("volume") {
                    Some(ConfigValue::Integer(v)) => ConfigValue::Integer(v / 10),
                    _ => ConfigValue::Integer(5),
                };
                MigrationOutput {
                    writes: vec![("gain".into(), gain)],
                    deletes: vec!["volume".into()],
                    new_schema: Schema {
                        namespace: "services/voice/tts".into(),
                        schema_version: 2,
                        fields: vec![SchemaField {
                            key: "gain".into(),
                            value_type: "integer".into(),
                            required: true,
                            default: Some(ConfigValue::Integer(5)),
                            min: Some(0),
                            max: Some(10),
                        }],
                    },
                }
            }),
        });
        let new_rev = svc
            .migrate_namespace(&writer, "services/voice/tts")
            .unwrap();
        // 数据已迁移
        let gain = svc.get(&reader, &ns, "gain").unwrap();
        assert_eq!(gain.value, ConfigValue::Integer(7), "70/10=7");
        let old = svc.get(&reader, &ns, "volume");
        assert!(
            matches!(old, Err(ConfigError::KeyNotFound(_))),
            "旧字段应已删除"
        );
        // 新 schema 生效：再提交 gain 合法、volume 拒绝
        svc.commit(
            &writer,
            ConfigTransaction {
                namespace: ns.clone(),
                expected_revision: new_rev,
                writes: vec![("gain".into(), ConfigValue::Integer(8))],
                deletes: vec![],
            },
        )
        .unwrap();
        assert!(
            svc.commit(
                &writer,
                ConfigTransaction {
                    namespace: ns,
                    expected_revision: new_rev + 1,
                    writes: vec![("volume".into(), ConfigValue::Integer(1))],
                    deletes: vec![],
                },
            )
            .is_err(),
            "旧字段在新 schema 下应拒绝"
        );
        // outbox 记录迁移
        let outbox = svc.drain_outbox();
        assert!(
            outbox
                .iter()
                .any(|e| e.keys.iter().any(|k| k.contains("schema migration")))
        );
    }

    #[test]
    fn migrate_without_registration_rejected() {
        let (svc, writer, _r) = svc_with_caps();
        svc.register_schema(Schema {
            namespace: "services/voice/tts".into(),
            schema_version: 1,
            fields: vec![],
        })
        .unwrap();
        let err = svc
            .migrate_namespace(&writer, "services/voice/tts")
            .unwrap_err();
        assert!(matches!(err, ConfigError::ValidationFailed(_)));
    }

    #[test]
    fn json_file_provider_atomic_roundtrip() {
        let dir = crate::testutil::tempdir();
        let path = dir.join("config.json");
        let prov = JsonFileProvider::open(path.clone()).unwrap();
        let ns = "services/executor";
        let tx = ConfigTransaction {
            namespace: NamespacePath::new(ns).unwrap(),
            expected_revision: 0,
            writes: vec![("direct_channel_enabled".into(), ConfigValue::Bool(true))],
            deletes: vec![],
        };
        prov.commit(&tx).unwrap();
        // 重开读回
        let prov2 = JsonFileProvider::open(path).unwrap();
        let v = prov2.get(ns, "direct_channel_enabled").unwrap().unwrap();
        assert_eq!(v.value, ConfigValue::Bool(true));
    }

    #[test]
    fn no_permission_denied() {
        let mut caps = CapabilityStore::new();
        let outsider = caps
            .grant(
                "other",
                Rights::ConfigWrite {
                    namespace: "services/other".into(),
                },
            )
            .unwrap();
        let svc = ConfigService::new(Box::new(MemoryProvider::new()), std::sync::Arc::new(caps));
        let tx = ConfigTransaction {
            namespace: NamespacePath::new("services/voice").unwrap(),
            expected_revision: 0,
            writes: vec![("k".into(), ConfigValue::Bool(true))],
            deletes: vec![],
        };
        let err = svc.commit(&outsider, tx).unwrap_err();
        assert!(matches!(err, ConfigError::PermissionDenied(_)));
    }
}
