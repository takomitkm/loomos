//! 最小 LAN HTTP 客户端/服务端 + 板端 HTTP 设备适配器（8091/8092/8093 契约）。
//!
//! 契约来源（旧板实测，2026-10-30）：
//! - 8091 face：POST `/api/recognize-camera` 空 body →
//!   `{"entry":{"result":{"identities":[...],"face_count":N,"presentation_attack_detection":
//!   {"status":...,"security_calibrated":...}}}}`；503 = 暂无新鲜摄像头画面；
//!   GET `/api/health` → `{"status":..,"pad_security_calibrated":bool}`；
//! - 8092 view：GET `/api/status` → `{"ready":bool,"session":..}`；
//! - 8093 recording：POST `/api/mode` `{"mode":"off"|"auto"|"manual"}` 回读校验；
//!   GET `/api/status` → `{"mode":..,"state":..,"queue":..}`。
//!
//! 纪律：绝不伪造 Origin/认证头；503 → NoFreshFrame；stale 帧不得用于授权。

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;
use thiserror::Error;

use self::mini_http_server::{HttpResponse, handle_conn_minimal};

#[derive(Error, Debug, Clone)]
pub enum MiniHttpError {
    #[error("连接失败：{0}")]
    Connect(String),
    #[error("响应解析失败：{0}")]
    Parse(String),
    #[error("IO 错误：{0}")]
    Io(String),
}

/// 极简 HTTP/1.1 客户端（LAN 原型用，无 TLS——板端服务本就无鉴权，仅 Origin 检查）。
pub fn mini_http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Value), MiniHttpError> {
    let addr = format!("{host}:{port}");
    let mut stream =
        TcpStream::connect(&addr).map_err(|e| MiniHttpError::Connect(format!("{addr}: {e}")))?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| MiniHttpError::Io(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| MiniHttpError::Io(e.to_string()))?;

    let body = body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(req.as_bytes())
        .map_err(|e| MiniHttpError::Io(e.to_string()))?;

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(|e| MiniHttpError::Io(e.to_string()))?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| MiniHttpError::Parse(format!("状态行: {status_line:?}")))?;

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|e| MiniHttpError::Io(e.to_string()))?;
        let l = line.trim_end();
        if l.is_empty() {
            break;
        }
        if let Some(v) = l.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body_bytes)
            .map_err(|e| MiniHttpError::Io(e.to_string()))?;
    }
    let value: Value = serde_json::from_slice(&body_bytes).unwrap_or(Value::String(
        String::from_utf8_lossy(&body_bytes).to_string(),
    ));
    Ok((status, value))
}

/// 极简 HTTP 服务端（测试/演示用）：线程池 1 线程逐连接处理。
pub struct MiniHttpServer {
    pub addr: String,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl MiniHttpServer {
    /// 启动后台线程。handler 返回 (status, json body)。
    pub fn spawn<F>(handler: F) -> Result<Self, MiniHttpError>
    where
        F: Fn(&str, &str, &str) -> (u16, Value) + Send + Sync + 'static,
    {
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| MiniHttpError::Connect(e.to_string()))?;
        let addr = listener
            .local_addr()
            .map_err(|e| MiniHttpError::Connect(e.to_string()))?
            .to_string();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let handler = std::sync::Arc::new(handler);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                if stop2.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = stream else { continue };
                let handler = handler.clone();
                std::thread::spawn(move || {
                    let _ = handle_conn(&mut stream, |m, p, b| handler(m, p, b));
                });
            }
        });
        Ok(Self { addr, stop })
    }

    pub fn port(&self) -> u16 {
        self.addr
            .rsplit(':')
            .next()
            .and_then(|p| p.parse().ok())
            .unwrap_or(0)
    }

    pub fn stop(&self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

fn handle_conn<F>(stream: &mut TcpStream, handler: F) -> std::io::Result<()>
where
    F: Fn(&str, &str, &str) -> (u16, Value),
{
    handle_conn_minimal(stream, |req| {
        let (status, value) = handler(&req.method, &req.path, &req.body);
        HttpResponse {
            status,
            body: serde_json::to_string(&value).unwrap_or_default(),
        }
    })
}

/// 结构化请求/响应（控制台复用同一解析路径）。
pub mod mini_http_server {
    use super::*;

    #[derive(Debug, Clone)]
    pub struct HttpRequest {
        pub method: String,
        pub path: String,
        pub query: std::collections::BTreeMap<String, String>,
        pub body: String,
    }

    impl HttpRequest {
        pub fn query(&self, key: &str) -> Option<&str> {
            self.query.get(key).map(|s| s.as_str())
        }
    }

    pub struct HttpResponse {
        pub status: u16,
        pub body: String,
    }

    fn parse_request(stream: &mut TcpStream) -> std::io::Result<HttpRequest> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let full_path = parts.next().unwrap_or("").to_string();
        let (path, query) = match full_path.split_once('?') {
            Some((p, q)) => (
                p.to_string(),
                q.split('&')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
            None => (full_path, Default::default()),
        };
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line.trim_end().is_empty() {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            reader.read_exact(&mut body)?;
        }
        Ok(HttpRequest {
            method,
            path,
            query,
            body: String::from_utf8_lossy(&body).to_string(),
        })
    }

    /// 结构化 HTTP 处理循环（单连接）。
    pub fn handle_conn_minimal<F>(stream: &mut TcpStream, handler: F) -> std::io::Result<()>
    where
        F: Fn(&HttpRequest) -> HttpResponse,
    {
        let req = parse_request(stream)?;
        let resp = handler(&req);
        let reason = match resp.status {
            200 => "OK",
            201 => "Created",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            503 => "Service Unavailable",
            _ => "Internal",
        };
        let out = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n{body}",
            status = resp.status,
            len = resp.body.len(),
            body = resp.body,
        );
        stream.write_all(out.as_bytes())?;
        stream.flush()?;
        Ok(())
    }
}

// ---------------- 板端 HTTP 适配器 ----------------

use crate::core::{
    DeviceCapability, DeviceDescriptor, DeviceError, DeviceHealth, DeviceProvider, OpOutcome,
    ProviderId,
};

/// 板端服务适配器：把 8091/8092/8093 暴露成 DeviceProvider。
/// 映射质量声明：对旧板 API 为 `Partial`（只覆盖本系统用到的子集）。
pub struct HttpBoardAdapter {
    id: ProviderId,
    face_port: u16,   // 8091 wcis-face-photo
    view_port: u16,   // 8092 wcis-sdi-vpu-view
    record_port: u16, // 8093 wcis-face-recording
    host: String,
    timeout: Duration,
}

impl HttpBoardAdapter {
    pub fn new(host: &str, face_port: u16, view_port: u16, record_port: u16) -> Self {
        Self {
            id: "http-board".into(),
            host: host.to_string(),
            face_port,
            view_port,
            record_port,
            timeout: Duration::from_secs(60),
        }
    }

    pub fn for_test(face_port: u16, view_port: u16, record_port: u16) -> Self {
        Self::new("127.0.0.1", face_port, view_port, record_port)
    }
}

impl DeviceProvider for HttpBoardAdapter {
    fn id(&self) -> ProviderId {
        self.id.clone()
    }

    fn discover(&mut self) -> Vec<DeviceDescriptor> {
        vec![
            DeviceDescriptor {
                id: "face.board".into(),
                provider: self.id.clone(),
                kind: "face".into(),
                version: 1,
                capabilities: vec![DeviceCapability::FaceMatch, DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "view.board".into(),
                provider: self.id.clone(),
                kind: "camera".into(),
                version: 1,
                capabilities: vec![DeviceCapability::Status],
            },
            DeviceDescriptor {
                id: "recorder.board".into(),
                provider: self.id.clone(),
                kind: "recorder".into(),
                version: 1,
                capabilities: vec![DeviceCapability::RecordMode, DeviceCapability::Status],
            },
        ]
    }

    fn execute(
        &mut self,
        device: &str,
        op: &str,
        params: &serde_json::Value,
    ) -> Result<OpOutcome, DeviceError> {
        match (device, op) {
            ("face.board", "face_match") => {
                // 旧板契约：POST /api/recognize-camera 空 body
                let (status, v) = mini_http_request(
                    &self.host,
                    self.face_port,
                    "POST",
                    "/api/recognize-camera",
                    Some(""),
                    self.timeout,
                )
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
                if status == 503 {
                    return Err(DeviceError::NoFreshFrame(device.to_string()));
                }
                if status != 200 {
                    return Err(DeviceError::Failed(format!("face 8091 HTTP {status}")));
                }
                // 8091 返回嵌套 entry.result
                let result = v
                    .get("entry")
                    .and_then(|e| e.get("result"))
                    .cloned()
                    .unwrap_or(v);
                Ok(OpOutcome::Done(result))
            }
            ("face.board", "query") => {
                let (status, v) = mini_http_request(
                    &self.host,
                    self.face_port,
                    "GET",
                    "/api/health",
                    None,
                    self.timeout,
                )
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
                if status != 200 {
                    return Err(DeviceError::Failed(format!("face health HTTP {status}")));
                }
                Ok(OpOutcome::Done(v))
            }
            ("view.board", "query") => {
                let (status, v) = mini_http_request(
                    &self.host,
                    self.view_port,
                    "GET",
                    "/api/status",
                    None,
                    self.timeout,
                )
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
                if status != 200 {
                    return Err(DeviceError::Failed(format!("view 8092 HTTP {status}")));
                }
                Ok(OpOutcome::Done(v))
            }
            ("recorder.board", "record_mode") => {
                let mode = params
                    .get("mode")
                    .and_then(|m| m.as_str())
                    .ok_or_else(|| DeviceError::BadParams("mode 必填".into()))?;
                if !matches!(mode, "off" | "auto" | "manual") {
                    return Err(DeviceError::BadParams(
                        "Mode must be off, auto or manual".into(),
                    ));
                }
                let body = serde_json::json!({"mode": mode}).to_string();
                let (status, v) = mini_http_request(
                    &self.host,
                    self.record_port,
                    "POST",
                    "/api/mode",
                    Some(&body),
                    self.timeout,
                )
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
                if status != 200 {
                    return Err(DeviceError::Failed(format!(
                        "record 8093 HTTP {status}: {v}"
                    )));
                }
                // 回读校验（旧板行为：回读 mode 不符即报错）
                if v.get("mode").and_then(|m| m.as_str()) != Some(mode) {
                    return Err(DeviceError::Failed("8093 回读 mode 不符".into()));
                }
                Ok(OpOutcome::Done(v))
            }
            ("recorder.board", "query") => {
                let (status, v) = mini_http_request(
                    &self.host,
                    self.record_port,
                    "GET",
                    "/api/status",
                    None,
                    self.timeout,
                )
                .map_err(|e| DeviceError::Failed(e.to_string()))?;
                if status != 200 {
                    return Err(DeviceError::Failed(format!("record 8093 HTTP {status}")));
                }
                Ok(OpOutcome::Done(v))
            }
            _ => Err(DeviceError::UnsupportedOp(
                device.to_string(),
                op.to_string(),
            )),
        }
    }

    fn health(&self, _device: &str) -> Result<DeviceHealth, DeviceError> {
        Ok(DeviceHealth::Ok)
    }

    fn telemetry(&self, device: &str) -> Result<serde_json::Value, DeviceError> {
        if device == "recorder.board" {
            let (_, v) = mini_http_request(
                &self.host,
                self.record_port,
                "GET",
                "/api/storage",
                None,
                self.timeout,
            )
            .map_err(|e| DeviceError::Failed(e.to_string()))?;
            return Ok(v);
        }
        Ok(serde_json::json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 板端 mock：复刻 8091/8092/8093 契约（含 503、mode 校验、回读）。
    fn board_handler(method: &str, path: &str, body: &str) -> (u16, Value) {
        match (method, path) {
            ("POST", "/api/recognize-camera") => {
                if body.is_empty() {
                    (
                        200,
                        json!({"entry": {"result": {
                            "identities": [{"name": "张三"}],
                            "face_count": 1,
                            "presentation_attack_detection": {"status": "model_live", "security_calibrated": false},
                        }}}),
                    )
                } else {
                    (400, json!({"error": "empty body expected"}))
                }
            }
            ("GET", "/api/health") => (
                200,
                json!({"status": "ok", "pad_security_calibrated": false}),
            ),
            ("GET", "/api/status") => (
                200,
                json!({"ready": true, "session": "live-1", "mode": "off", "state": "idle", "queue": 0}),
            ),
            ("POST", "/api/mode") => {
                let v: Value = serde_json::from_str(body).unwrap_or(json!({}));
                let mode = v.get("mode").and_then(|m| m.as_str()).unwrap_or("");
                if !matches!(mode, "off" | "auto" | "manual") {
                    (400, json!({"error": "Mode must be off, auto or manual"}))
                } else {
                    (200, json!({"mode": mode}))
                }
            }
            ("GET", "/api/storage") => (200, json!({"trash_count": 26})),
            _ => (404, json!({"error": "not found"})),
        }
    }

    #[test]
    fn adapter_talks_board_contract() {
        // 三个端口都指到同一 mock（端口隔离由 path 区分不够，这里直接各起一个服务）
        let face = MiniHttpServer::spawn(|m, p, b| {
            if p == "/api/status" {
                return (200, json!({"ready": true}));
            }
            board_handler(m, p, b)
        })
        .unwrap();
        let view = MiniHttpServer::spawn(board_handler).unwrap();
        let record = MiniHttpServer::spawn(board_handler).unwrap();

        let mut adapter = HttpBoardAdapter::for_test(face.port(), view.port(), record.port());
        // face_match：嵌套 entry.result 解包
        let out = adapter
            .execute("face.board", "face_match", &json!({}))
            .unwrap();
        if let OpOutcome::Done(v) = out {
            assert_eq!(v["face_count"], 1);
            assert_eq!(v["presentation_attack_detection"]["status"], "model_live");
        } else {
            panic!("应为 Done");
        }
        // record_mode 非法值 → 板端 400 契约
        let err = adapter
            .execute("recorder.board", "record_mode", &json!({"mode": "on"}))
            .unwrap_err();
        assert!(err.to_string().contains("Mode must be off, auto or manual"));
        // 正常值回读校验通过
        adapter
            .execute("recorder.board", "record_mode", &json!({"mode": "auto"}))
            .unwrap();
        face.stop();
        view.stop();
        record.stop();
    }

    #[test]
    fn mini_http_roundtrip() {
        let srv = MiniHttpServer::spawn(|_m, _p, _b| (200, json!({"ok": true}))).unwrap();
        let (status, v) = mini_http_request(
            "127.0.0.1",
            srv.port(),
            "GET",
            "/x",
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(status, 200);
        assert_eq!(v["ok"], true);
        srv.stop();
    }
}
