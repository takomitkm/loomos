//! 事件与 Outbox（已发生的事实及其序号；control/events.py + control/outbox.py 的 Rust 版）。
//!
//! 纪律（architecture/04 §4.8）：发布事件必须在事务提交之后；tokio broadcast /
//! mpsc 只作进程内通知，不作为持久事件来源。这里 EventLog 是持久来源（JSONL），
//! Bus 是进程内通知。

use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    ConfigChanged,
    LeaseGranted,
    LeaseReleased,
    LeaseRevoked,
    LeaseOrphaned,
    CapabilityGranted,
    CapabilityRevoked,
    ComponentStateChange,
    FailureClassified,
    RecoveryAction,
    OperationSubmitted,
    OperationUpdated,
    WakeDetected,
    Utterance,
    IntentParsed,
    ActionExecuted,
    ActionRejected,
    TtsProduced,
    FaultReported,
    DeviceStateChange,
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            EventKind::ConfigChanged => "config.changed",
            EventKind::LeaseGranted => "lease.granted",
            EventKind::LeaseReleased => "lease.released",
            EventKind::LeaseRevoked => "lease.revoked",
            EventKind::LeaseOrphaned => "lease.orphaned",
            EventKind::CapabilityGranted => "capability.granted",
            EventKind::CapabilityRevoked => "capability.revoked",
            EventKind::ComponentStateChange => "component.state",
            EventKind::FailureClassified => "failure.classified",
            EventKind::RecoveryAction => "recovery.action",
            EventKind::OperationSubmitted => "operation.submitted",
            EventKind::OperationUpdated => "operation.updated",
            EventKind::WakeDetected => "voice.wake",
            EventKind::Utterance => "voice.utterance",
            EventKind::IntentParsed => "nlu.intent",
            EventKind::ActionExecuted => "action.executed",
            EventKind::ActionRejected => "action.rejected",
            EventKind::TtsProduced => "tts.produced",
            EventKind::FaultReported => "fault.reported",
            EventKind::DeviceStateChange => "device.state",
        };
        f.write_str(s)
    }
}

/// 事件 envelope（control/events.py 契约的精简版：producer + producer_seq 全局单调）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub producer_id: String,
    pub producer_seq: u64,
    /// Unix 时间（毫秒）。
    pub wall_time_ms: u64,
    pub kind: EventKind,
    pub source: String,
    pub payload: serde_json::Value,
}

/// 进程内事件总线。
#[derive(Default)]
pub struct Bus {
    senders: Mutex<Vec<Sender<Event>>>,
}

impl Bus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = channel();
        self.senders.lock().unwrap().push(tx);
        rx
    }

    pub fn publish(&self, event: &Event) {
        let mut senders = self.senders.lock().unwrap();
        senders.retain(|tx| tx.send(event.clone()).is_ok());
    }
}

/// 持久事件日志（JSONL 追加；read_after 游标读取）。
pub struct EventLog {
    producer_id: String,
    next_seq: u64,
    path: Option<PathBuf>,
    file: Option<File>,
    bus: Bus,
    memory: Vec<Event>,
    memory_cap: usize,
}

impl EventLog {
    pub fn in_memory(producer_id: &str) -> Self {
        Self {
            producer_id: producer_id.to_string(),
            next_seq: 1,
            path: None,
            file: None,
            bus: Bus::new(),
            memory: Vec::new(),
            memory_cap: 10_000,
        }
    }

    /// 带持久化：追加写 JSONL（崩溃后重放续 seq）。
    pub fn persistent(producer_id: &str, path: PathBuf) -> std::io::Result<Self> {
        let mut next_seq = 1u64;
        if path.exists() {
            let f = BufReader::new(File::open(&path)?);
            for line in f.lines() {
                let line = line?;
                if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                    next_seq = next_seq.max(ev.seq + 1);
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            producer_id: producer_id.to_string(),
            next_seq,
            path: Some(path),
            file: Some(file),
            bus: Bus::new(),
            memory: Vec::new(),
            memory_cap: 10_000,
        })
    }

    pub fn bus(&self) -> &Bus {
        &self.bus
    }

    /// 持久化路径（证据追溯用）。
    pub fn path(&self) -> Option<&PathBuf> {
        self.path.as_ref()
    }

    /// 追加事件：分配 producer_seq，持久化（先落盘），再进程内广播。
    pub fn append(&mut self, kind: EventKind, source: &str, payload: serde_json::Value) -> Event {
        let seq = self.next_seq;
        self.next_seq += 1;
        let event = Event {
            seq,
            producer_id: self.producer_id.clone(),
            producer_seq: seq,
            wall_time_ms: now_unix_ms(),
            kind,
            source: source.to_string(),
            payload,
        };
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{}", serde_json::to_string(&event).unwrap_or_default());
        }
        self.memory.push(event.clone());
        if self.memory.len() > self.memory_cap {
            self.memory.remove(0);
        }
        self.bus.publish(&event);
        event
    }

    /// 游标读取（`after` 之后的最多 `limit` 条；control/service.py `/v1/events` 契约）。
    pub fn read_after(&self, after: u64, limit: usize) -> Vec<Event> {
        self.memory
            .iter()
            .filter(|e| e.seq > after)
            .take(limit)
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.memory.len()
    }

    pub fn is_empty(&self) -> bool {
        self.memory.is_empty()
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seq_monotonic_and_read_after() {
        let mut log = EventLog::in_memory("test");
        for i in 0..10 {
            log.append(EventKind::Utterance, "voice", serde_json::json!({"i": i}));
        }
        let after5 = log.read_after(5, 3);
        assert_eq!(after5.len(), 3);
        assert_eq!(after5[0].seq, 6);
        assert_eq!(after5[2].seq, 8);
        // producer_seq 与 seq 一致且不重复
        let seqs: Vec<u64> = log.read_after(0, 100).iter().map(|e| e.seq).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn persistent_log_survives_reopen() {
        let dir = crate::testutil::tempdir();
        let path = dir.join("events.jsonl");
        {
            let mut log = EventLog::persistent("test", path.clone()).unwrap();
            log.append(
                EventKind::WakeDetected,
                "kws",
                serde_json::json!({"kw": "小克"}),
            );
            log.append(
                EventKind::Utterance,
                "asr",
                serde_json::json!({"text": "打开灯"}),
            );
        }
        let mut log = EventLog::persistent("test", path).unwrap();
        let ev = log.append(EventKind::IntentParsed, "nlu", serde_json::json!({}));
        // 重开后 seq 续号不覆盖
        assert_eq!(ev.seq, 3);
    }

    #[test]
    fn bus_delivers_to_subscribers() {
        let mut log = EventLog::in_memory("test");
        let rx = log.bus().subscribe();
        log.append(
            EventKind::ActionExecuted,
            "executor",
            serde_json::json!({"ok": true}),
        );
        let got = rx.recv().unwrap();
        assert_eq!(got.kind, EventKind::ActionExecuted);
    }
}
