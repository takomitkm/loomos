//! 链接 WAMR 运行时静态库（vmlib）。
//!
//! 纪律（2026-11-20 修订）：本 crate 必须始终可编译（workspace 门禁依赖它）——
//! - 未启用 `wamr` feature：不链接 vmlib，native 模式照常可用；
//! - 启用 `wamr` 且未设 IWASM_LIB_DIR：**此时才 panic**（明确失败优于静默链接失败）。

fn main() {
    println!("cargo:rerun-if-env-changed=IWASM_LIB_DIR");
    let want_wamr = std::env::var_os("CARGO_FEATURE_WAMR").is_some();
    match std::env::var("IWASM_LIB_DIR") {
        Ok(dir) => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=iwasm");
            // WAMR windows 平台静态库的系统依赖（CI 实测）
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
                println!("cargo:rustc-link-lib=bcrypt");
                println!("cargo:rustc-link-lib=pathcch");
            }
        }
        Err(_) if want_wamr => {
            panic!("wamr feature 需要 IWASM_LIB_DIR 指向 libiwasm.a 所在目录")
        }
        Err(_) => {
            println!(
                "cargo:warning=nlu-bench: IWASM_LIB_DIR 未设置——native 模式可用；interp/aot 需要 --features wamr"
            );
        }
    }
}
