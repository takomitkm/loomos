#![forbid(unsafe_code)]

//! loomosd 库入口：装配（System）、语音管线（voice）、控制台（console）。

pub mod console;
pub mod system;
pub mod voice;

pub use system::{System, SystemConfig};
pub use voice::{TurnOutcome, VoicePipeline};
