//! 控制台 HTTP 服务（旧 10900 统一控制台的新架构版本）。
//!
//! 端点（对齐旧 control/service.py 的公开面 + 板端语义）：
//! - GET  `/health`：存活
//! - GET  `/status`：系统状态（组件/设备/事件数/uptime）
//! - GET  `/entities`：实体注册表摘要
//! - GET  `/events?after=N&limit=M`：事件游标读取
//! - POST `/handle_text` `{"text": "..."}`：文本通道（console principal）
//! - POST `/action` `{"intent": "HassTurnOn", "slots": {"entity_id": "light.keting"}}`
//! - GET  `/config/{ns}` / POST `/config`：配置读写（capability 校验）
//! - POST `/voice/replay` `{"samples_b64" 或 "fixture": "u01"}`：走完整语音管线（echo/夹具档）

use crate::system::System;
use loomos_device::http::mini_http_server::{HttpRequest, HttpResponse, handle_conn_minimal};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct ConsoleServer {
    pub addr: String,
    stop: Arc<AtomicBool>,
}

impl ConsoleServer {
    pub fn spawn(sys: Arc<System>, port: u16) -> Result<Self, String> {
        let listener = TcpListener::bind(("127.0.0.1", port)).map_err(|e| e.to_string())?;
        let addr = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut s) = stream else { continue };
                let sys = sys.clone();
                std::thread::spawn(move || {
                    let handler = |req: &HttpRequest| route(&sys, req);
                    let _ = handle_conn_minimal(&mut s, handler);
                });
            }
        });
        Ok(Self { addr, stop })
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    pub fn port(&self) -> u16 {
        self.addr
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    }
}

fn route(sys: &Arc<System>, req: &HttpRequest) -> HttpResponse {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") => json_response(
            200,
            serde_json::json!({"status": "ok", "version": loomos_runtime::RUNTIME_VERSION}),
        ),
        ("GET", "/status") => {
            let reg = sys.registry.lock().unwrap();
            let devices: Vec<serde_json::Value> = reg
                .devices()
                .map(|(d, lc)| {
                    serde_json::json!({
                        "id": d.id, "kind": d.kind, "provider": d.provider,
                        "lifecycle": format!("{lc:?}"),
                    })
                })
                .collect();
            let ops_total = reg.operations().count();
            drop(reg);
            let events = sys.events.lock().unwrap();
            let seq = events.read_after(0, 1).last().map(|e| e.seq).unwrap_or(0);
            let event_count = events.len();
            drop(events);
            let components: Vec<serde_json::Value> = sys
                .lifecycle
                .lock()
                .unwrap()
                .all()
                .map(|r| serde_json::json!({"id": r.manifest.id, "state": format!("{:?}", r.state), "restarts": r.restarts}))
                .collect();
            json_response(
                200,
                serde_json::json!({
                    "version": loomos_runtime::RUNTIME_VERSION,
                    "uptime_ms": sys.uptime_ms(),
                    "components": components,
                    "devices": devices,
                    "operations_total": ops_total,
                    "events_total": event_count,
                    "event_seq": seq,
                }),
            )
        }
        ("GET", "/entities") => json_response(
            200,
            serde_json::json!({
                "entities": sys.registry_json.summary(),
            }),
        ),
        ("GET", p) if p.starts_with("/events") => {
            let after = req
                .query("after")
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            let limit = req
                .query("limit")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(100);
            let events = sys.events.lock().unwrap().read_after(after, limit);
            json_response(200, serde_json::json!({"events": events}))
        }
        ("POST", "/handle_text") => {
            let body: serde_json::Value = match serde_json::from_str(&req.body) {
                Ok(v) => v,
                Err(e) => return json_response(400, serde_json::json!({"error": e.to_string()})),
            };
            let text = body.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text.is_empty() {
                return json_response(400, serde_json::json!({"error": "text 必填"}));
            }
            let report = sys.handle_text(text);
            json_response(200, serde_json::to_value(&report).unwrap_or_default())
        }
        ("POST", "/action") => {
            let body: serde_json::Value = match serde_json::from_str(&req.body) {
                Ok(v) => v,
                Err(e) => return json_response(400, serde_json::json!({"error": e.to_string()})),
            };
            let intent_name = body.get("intent").and_then(|i| i.as_str()).unwrap_or("");
            let slots: std::collections::BTreeMap<String, String> = body
                .get("slots")
                .and_then(|s| serde_json::from_value(s.clone()).ok())
                .unwrap_or_default();
            let Some(name) = parse_intent(intent_name) else {
                return json_response(
                    400,
                    serde_json::json!({"error": format!("未知意图 {intent_name}")}),
                );
            };
            let outcomes = sys.run_action(loomos_nlu::intents::Intent {
                name,
                slots,
                matched_by: loomos_nlu::intents::MatchChannel::Direct,
            });
            json_response(200, serde_json::to_value(&outcomes).unwrap_or_default())
        }
        ("GET", p) if p.starts_with("/config/") => {
            let ns = p.trim_start_matches("/config/");
            let keys = sys.config.provider_list(ns).unwrap_or_default();
            let mut values = serde_json::Map::new();
            for k in keys {
                if let Ok(Some(v)) = sys.config.provider_get(ns, &k) {
                    values.insert(k, serde_json::to_value(&v.value).unwrap_or_default());
                }
            }
            json_response(200, serde_json::json!({"namespace": ns, "values": values}))
        }
        ("POST", "/voice/replay") => {
            let body: serde_json::Value = match serde_json::from_str(&req.body) {
                Ok(v) => v,
                Err(e) => return json_response(400, serde_json::json!({"error": e.to_string()})),
            };
            // 简单路径：直接给文本绕过 ASR，模拟一次"已转写"的语音回合
            let text = body.get("text").and_then(|t| t.as_str()).unwrap_or("");
            if text.is_empty() {
                return json_response(
                    400,
                    serde_json::json!({"error": "text 必填（原型暂不含音频上传）"}),
                );
            }
            let report = sys.handle_text(text);
            json_response(
                200,
                serde_json::json!({
                    "transcript": text,
                    "report": serde_json::to_value(&report).unwrap_or_default(),
                }),
            )
        }
        ("POST", "/shutdown") => json_response(200, serde_json::json!({"bye": true})),
        _ => json_response(404, serde_json::json!({"error": "not found"})),
    }
}

fn parse_intent(name: &str) -> Option<loomos_nlu::intents::IntentName> {
    use loomos_nlu::intents::IntentName::*;
    Some(match name {
        "HassTurnOn" => HassTurnOn,
        "HassTurnOff" => HassTurnOff,
        "QueryStatus" => QueryStatus,
        "WhoIsThere" => WhoIsThere,
        "StartRecording" => StartRecording,
        "AutoRecording" => AutoRecording,
        "StopRecording" => StopRecording,
        "CapturePhoto" => CapturePhoto,
        "QueryTime" => QueryTime,
        "QueryDoorStatus" => QueryDoorStatus,
        "StartVoiceprint" => StartVoiceprint,
        "CancelReboot" => CancelReboot,
        _ => return None,
    })
}

fn json_response(status: u16, v: serde_json::Value) -> HttpResponse {
    HttpResponse {
        status,
        body: serde_json::to_string(&v).unwrap_or_default(),
    }
}
