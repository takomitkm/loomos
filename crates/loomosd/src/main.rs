#![forbid(unsafe_code)]

//! loomosd 服务器入口：装配原型 + 启动控制台。
//!
//! 用法：
//!   loomosd [--data-dir DIR] [--speech-dir DIR] [--port 10900]
//! 启动后：
//!   GET http://127.0.0.1:10900/status
//!   POST /handle_text {"text":"小克，打开客厅的灯"}

use loomosd::console::ConsoleServer;
use loomosd::system::{System, SystemConfig};

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let mut cfg = SystemConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir" => {
                cfg.data_dir = args.next().map(std::path::PathBuf::from);
            }
            "--speech-dir" => {
                cfg.speech_dir = args.next().map(std::path::PathBuf::from);
            }
            "--port" => {
                cfg.console_port = args.next().and_then(|p| p.parse().ok()).unwrap_or(10_900);
            }
            other => {
                eprintln!("未知参数 {other}");
                eprintln!("用法: loomosd [--data-dir DIR] [--speech-dir DIR] [--port 10900]");
                std::process::exit(2);
            }
        }
    }

    let sys = match System::assemble(cfg.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("装配失败: {e}");
            std::process::exit(1);
        }
    };
    let console = match ConsoleServer::spawn(sys.clone(), cfg.console_port) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("控制台启动失败: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        addr = %console.addr,
        version = loomos_runtime::RUNTIME_VERSION,
        "loomosd started; try GET http://{}/health",
        console.addr
    );
    println!(
        "loomosd listening on http://{} (POST /handle_text)",
        console.addr
    );

    // Ctrl-C 退出
    let _ = console;
    let _ = sys;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
