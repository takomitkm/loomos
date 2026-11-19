//! System 装配：把 runtime / config / device / nlu / executor / voice 接成一台完整原型。
//!
//! 装配即配置检查（architecture/05 §5.2 精神）：
//! - 组件 Manifest 逐个注册 + 依赖/能力检查；
//! - capability 按组件最小授权（不给全局 Admin 给业务组件——voice 演示用例外已注记）；
//! - 设备全 mock 提供者；HTTP 板适配器可挂（演示/测试）。

use loomos_device::core::{DeviceLifecycle, DeviceRegistry};
use loomos_device::mock::{MockFaults, MockHomeProvider};
use loomos_executor::{Executor, ExecutorConfig};
use loomos_home::actions::ActionRegistry;
use loomos_home::voiceprint::MockVoiceprint;
use loomos_nlu::grammar::RuleNlu;
use loomos_nlu::registry::EntityRegistry;
use loomos_runtime::capability::{CapabilityStore, Rights, Token};
use loomos_runtime::component::{ComponentManifest, LifecycleManager, LifecycleState};
use loomos_runtime::config::{
    ConfigService, ConfigTransaction, ConfigValue, JsonFileProvider, MemoryProvider, NamespacePath,
    Schema, SchemaField,
};
use loomos_runtime::event::{EventKind, EventLog};
use loomos_runtime::supervisor::{RecoveryPolicy, Supervisor};
use loomos_vision::provider::VisionMode;
use std::sync::{Arc, Mutex};

/// 系统装配配置。
#[derive(Debug, Clone)]
pub struct SystemConfig {
    /// 数据目录（配置/事件/语音落盘）。None = 全内存。
    pub data_dir: Option<std::path::PathBuf>,
    /// 语音输出目录（对齐旧板 /mnt/data/klar/speech）。
    pub speech_dir: Option<std::path::PathBuf>,
    /// mock 故障注入（演示四道闸/挂死/离线语义）。
    pub faults: MockFaults,
    /// 视频路装配模式（可拆卸总开关）。
    pub vision: VisionMode,
    /// console 绑定端口（0 = 随机）。
    pub console_port: u16,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            data_dir: None,
            speech_dir: None,
            faults: MockFaults::default(),
            vision: VisionMode::default(),
            console_port: 10_900,
        }
    }
}

/// 一台装配好的 loomos 原型。
pub struct System {
    pub events: Arc<Mutex<EventLog>>,
    pub caps: Arc<CapabilityStore>,
    pub voice_token: Token,
    pub console_token: Token,
    pub registry: Mutex<DeviceRegistry>,
    pub executor: Executor,
    pub voiceprint: Mutex<MockVoiceprint>,
    pub lifecycle: Mutex<LifecycleManager>,
    pub supervisor: Mutex<Supervisor>,
    pub config: ConfigService,
    pub registry_json: EntityRegistry,
    pub started_at_ms: u64,
}

impl System {
    /// 装配一台全 mock、全自检的原型（默认档空机可跑全链）。
    pub fn assemble(cfg: SystemConfig) -> Result<Arc<Self>, String> {
        let events = match &cfg.data_dir {
            Some(dir) => {
                std::fs::create_dir_all(dir).map_err(|e| format!("创建数据目录失败: {e}"))?;
                EventLog::persistent("loomosd", dir.join("events.jsonl"))
                    .map_err(|e| format!("事件日志初始化失败: {e}"))?
            }
            None => EventLog::in_memory("loomosd"),
        };
        let events = Arc::new(Mutex::new(events));

        // ---- capability：按组件最小授权（不给业务组件全局 Admin）----
        let mut caps = CapabilityStore::new();
        let voice_token = caps
            .grant(
                "voice",
                Rights::DeviceCommand {
                    device: "light.keting".into(),
                },
            )
            .unwrap();
        for d in [
            "light.woshi",
            "camera.menkou",
            "switch.recorder",
            "camera.view",
            "lock.door",
            "speaker.tts",
            "system.host",
        ] {
            caps.grant("voice", Rights::DeviceCommand { device: d.into() })
                .unwrap();
        }
        let console_token = caps
            .grant(
                "console",
                Rights::DeviceCommand {
                    device: "light.keting".into(),
                },
            )
            .unwrap();
        for d in [
            "light.woshi",
            "camera.menkou",
            "switch.recorder",
            "camera.view",
            "lock.door",
            "speaker.tts",
            "system.host",
        ] {
            caps.grant("console", Rights::DeviceCommand { device: d.into() })
                .unwrap();
        }
        // installer 只能写 system/install 命名空间（演示权限边界）
        let _installer = caps
            .grant(
                "installer",
                Rights::ConfigWrite {
                    namespace: "system/install".into(),
                },
            )
            .unwrap();
        // voice/console 的配置读写（voice 写自己的 namespace；读 services 树）
        caps.grant(
            "voice",
            Rights::ConfigWrite {
                namespace: "services/voice".into(),
            },
        )
        .unwrap();
        caps.grant(
            "voice",
            Rights::ConfigRead {
                namespace: "services".into(),
            },
        )
        .unwrap();
        caps.grant(
            "console",
            Rights::ConfigRead {
                namespace: "services".into(),
            },
        )
        .unwrap();
        let caps = Arc::new(caps);

        // ---- 配置服务 ----
        let provider: Box<dyn loomos_runtime::config::ConfigProvider> = match &cfg.data_dir {
            Some(dir) => Box::new(
                JsonFileProvider::open(dir.join("config.json"))
                    .map_err(|e| format!("配置文件初始化失败: {e}"))?,
            ),
            None => Box::new(MemoryProvider::new()),
        };
        let config = ConfigService::new(provider, caps.clone());
        // voice 组件的 Schema + 默认值（声明式：组件自己声明 Schema，服务不理解业务语义）
        config
            .register_schema(Schema {
                namespace: "services/voice".into(),
                schema_version: 1,
                fields: vec![
                    SchemaField {
                        key: "hotword".into(),
                        value_type: "string".into(),
                        required: true,
                        default: Some(ConfigValue::String("小克".into())),
                        min: None,
                        max: None,
                    },
                    SchemaField {
                        key: "silence_dbfs".into(),
                        value_type: "integer".into(),
                        required: true,
                        default: Some(ConfigValue::Integer(-45)),
                        min: Some(-90),
                        max: Some(0),
                    },
                    SchemaField {
                        key: "wake_required".into(),
                        value_type: "bool".into(),
                        required: true,
                        default: Some(ConfigValue::Bool(true)),
                        min: None,
                        max: None,
                    },
                ],
            })
            .map_err(|e| e.to_string())?;
        // 用 voice_token 提交默认值（装配层授权voice Admin，见上）
        Self::seed_config(
            &config,
            &voice_token,
            "services/voice",
            vec![
                ("hotword", ConfigValue::String("小克".into())),
                ("silence_dbfs", ConfigValue::Integer(-45)),
                ("wake_required", ConfigValue::Bool(true)),
            ],
        );

        // ---- 设备注册表 ----
        let mut registry = DeviceRegistry::new(events.clone());
        let with_camera = matches!(cfg.vision, VisionMode::MockDevices);
        registry
            .register_provider(Box::new(MockHomeProvider::with_camera(
                cfg.faults.clone(),
                with_camera,
            )))
            .map_err(|e| e.to_string())?;
        // 视频路可拆卸装配（architecture/06：Device Provider 是标准插槽）
        match &cfg.vision {
            VisionMode::Detached | VisionMode::MockDevices => {}
            VisionMode::Pipeline(vc) => {
                registry
                    .register_provider(Box::new(loomos_vision::VisionProvider::new(
                        vc.clone(),
                        now_ms,
                    )))
                    .map_err(|e| e.to_string())?;
            }
            VisionMode::HttpBoard {
                host,
                face_port,
                view_port,
                record_port,
            } => {
                registry
                    .register_provider(Box::new(loomos_device::HttpBoardAdapter::new(
                        host,
                        *face_port,
                        *view_port,
                        *record_port,
                    )))
                    .map_err(|e| e.to_string())?;
            }
        }
        for d in [
            "light.keting",
            "light.woshi",
            "camera.menkou",
            "switch.recorder",
            "camera.view",
            "lock.door",
            "speaker.tts",
            "system.host",
        ] {
            // 设备可能因视频路拆卸而不存在——存在才激活
            if registry.device(d).is_some() {
                registry
                    .set_lifecycle(d, DeviceLifecycle::Active)
                    .map_err(|e| e.to_string())?;
            }
        }
        let registry = Mutex::new(registry);

        // ---- NLU / 执行器 / 声纹 ----
        let registry_json = EntityRegistry::default_home();
        let nlu = RuleNlu::new(registry_json.clone());
        let executor = Executor::new(nlu, ExecutorConfig::default());
        let voiceprint = Mutex::new(MockVoiceprint::new());

        // ---- 组件生命周期 + Supervisor ----
        let mut lifecycle = LifecycleManager::new();
        for m in [
            component_manifest("config-service", &[], &[]),
            component_manifest(
                "device-registry",
                &["config-service.v1"],
                &["device.command"],
            ),
            component_manifest(
                "voice-pipeline",
                &["nlu.v1", "executor.v1"],
                &["device.command", "config.write"],
            ),
            component_manifest(
                "executor",
                &["nlu.v1", "device-registry.v1"],
                &["device.command"],
            ),
            component_manifest("nlu", &[], &[]),
        ] {
            lifecycle.register(m).map_err(|e| e.to_string())?;
        }
        // 装配检查：provides 清单 + 授权能力
        let provided: Vec<String> = vec![
            "config-service.v1".into(),
            "device-registry.v1".into(),
            "nlu.v1".into(),
            "executor.v1".into(),
            "voice-pipeline.v1".into(),
        ];
        let granted: Vec<String> = vec!["device.command".into(), "config.write".into()];
        for c in lifecycle
            .all()
            .map(|r| r.manifest.id.clone())
            .collect::<Vec<_>>()
        {
            lifecycle
                .check_assembly(&c, &provided, &granted)
                .map_err(|e| format!("装配检查失败 {c}: {e}"))?;
            for st in [
                LifecycleState::Configured,
                LifecycleState::Ready,
                LifecycleState::Active,
            ] {
                lifecycle.transition(&c, st).map_err(|e| e.to_string())?;
            }
        }
        let lifecycle = Mutex::new(lifecycle);

        let mut supervisor = Supervisor::new(RecoveryPolicy::default());
        for c in [
            "config-service",
            "device-registry",
            "voice-pipeline",
            "executor",
            "nlu",
        ] {
            supervisor.register(c);
        }
        let supervisor = Mutex::new(supervisor);

        // ---- Linux Backend 探针（B 类 OS 代劳；施加后读回验证，如实上报）----
        let probe_budget = loomos_runtime::resource::CpuBudget {
            runtime_us: 30_000,
            period_us: 100_000,
            deadline_us: 90_000,
        };
        let probe_cores = loomos_runtime::resource::CoreSet::new(vec![0, 1], 4)
            .map_err(|e| format!("探测核集合非法: {e}"))?;
        let enforcement =
            loomos_runtime::platform::probe(Some(&probe_budget), 512 * 1024 * 1024, &probe_cores);
        for r in &enforcement {
            tracing::info!(item = %r.item, level = %r.level, evidence = %r.evidence, "enforcement probe");
        }

        events.lock().unwrap().append(
            EventKind::ComponentStateChange,
            "system",
            serde_json::json!({"note": "assembly complete", "components": 5}),
        );

        Ok(Arc::new(Self {
            events,
            caps,
            voice_token,
            console_token,
            registry,
            executor,
            voiceprint,
            lifecycle,
            supervisor,
            config,
            registry_json,
            started_at_ms: now_ms(),
        }))
    }

    fn seed_config(
        config: &ConfigService,
        token: &Token,
        ns: &str,
        pairs: Vec<(&str, ConfigValue)>,
    ) {
        let ns_path = match NamespacePath::new(ns) {
            Ok(p) => p,
            Err(_) => return,
        };
        let tx = ConfigTransaction {
            namespace: ns_path,
            expected_revision: 0,
            writes: pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            deletes: vec![],
        };
        let _ = config.commit(token, tx);
    }

    /// 便捷执行文本（console 路径）。
    pub fn handle_text(&self, text: &str) -> loomos_executor::ExecReport {
        let mut reg = self.registry.lock().unwrap();
        let vp = self.voiceprint.lock().unwrap();
        self.executor.handle_text(
            text,
            &mut reg,
            &self.caps,
            &self.voice_token,
            Some(&vp),
            &[],
            loomos_audio::RATE_INFERENCE,
        )
    }

    /// 便捷执行动作（console 直接调动作注册表）。
    pub fn run_action(
        &self,
        intent: loomos_nlu::intents::Intent,
    ) -> Vec<loomos_home::actions::ActionOutcome> {
        let mut reg = self.registry.lock().unwrap();
        ActionRegistry::run(
            &intent,
            &mut reg,
            "console",
            &self.caps,
            &self.console_token,
        )
    }

    pub fn uptime_ms(&self) -> u64 {
        now_ms().saturating_sub(self.started_at_ms)
    }
}

fn component_manifest(id: &str, requires: &[&str], capabilities: &[&str]) -> ComponentManifest {
    ComponentManifest {
        id: id.to_string(),
        version: 1,
        provides: vec![format!("{id}.v1")],
        requires: requires.iter().map(|s| s.to_string()).collect(),
        capabilities: capabilities.iter().map(|s| s.to_string()).collect(),
        resources: vec![],
        execution: vec!["same-address-space".into()],
        health: Default::default(),
        recovery: Default::default(),
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembly_runs_and_text_path_works() {
        let sys = System::assemble(SystemConfig::default()).unwrap();
        // 生命周期全部 Active
        let all_active = sys
            .lifecycle
            .lock()
            .unwrap()
            .all()
            .all(|r| r.state == LifecycleState::Active);
        assert!(all_active);
        // 文本路径
        let r = sys.handle_text("小克，打开客厅的灯");
        assert_eq!(r.speech, "客厅的灯开了。");
        // 设备状态已变
        let reg = sys.registry.lock().unwrap();
        let op = reg.devices().count();
        assert!(op >= 8);
        drop(reg);
        let status = sys.handle_text("小克，现在什么状态");
        assert!(status.speech.contains("录制模式"));
    }

    #[test]
    fn vision_detached_removes_camera_devices() {
        // 可拆卸验收：Detached 模式下相机/录制/画面设备不注册，
        // 涉及它们的意图会得到可观测失败（NoSuchDevice），其余功能不受影响。
        let sys = System::assemble(SystemConfig {
            vision: VisionMode::Detached,
            ..Default::default()
        })
        .unwrap();
        let reg = sys.registry.lock().unwrap();
        assert!(reg.device("camera.menkou").is_none(), "相机应被拆卸");
        assert!(reg.device("switch.recorder").is_none(), "录制应被拆卸");
        assert!(reg.device("camera.view").is_none(), "画面应被拆卸");
        assert!(reg.device("light.keting").is_some(), "灯控不受影响");
        drop(reg);
        let r = sys.handle_text("小克，打开客厅的灯");
        assert_eq!(r.speech, "客厅的灯开了。");
        let who = sys.handle_text("小克，看看是谁");
        assert!(
            who.actions.iter().any(|a| a.error.is_some()),
            "拆卸后相机意图应可观测失败"
        );
    }

    #[test]
    fn vision_pipeline_mode_serves_face_match() {
        let sys = System::assemble(SystemConfig {
            vision: VisionMode::Pipeline(Default::default()),
            ..Default::default()
        })
        .unwrap();
        let who = sys.handle_text("小克，看看是谁");
        assert_eq!(
            who.speech, "门口是张三。",
            "Pipeline 模式应经真实流水线识别"
        );
    }

    #[test]
    fn seeded_config_readable_via_schema() {
        let sys = System::assemble(SystemConfig::default()).unwrap();
        let ns = NamespacePath::new("services/voice").unwrap();
        let v = sys.config.get(&sys.voice_token, &ns, "hotword").unwrap();
        assert_eq!(v.value, ConfigValue::String("小克".into()));
    }
}
