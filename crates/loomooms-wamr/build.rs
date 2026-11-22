//! WAMR 链接（`wamr-link` feature 门控）。
//!
//! `IWASM_LIB_DIR` 环境变量指向 libiwasm.a / iwasm.lib 所在目录。
//! 未启用 feature 或未设环境变量 → 不链接（stub 模式，crate 始终可编译）。

fn main() {
    println!("cargo:rerun-if-env-changed=IWASM_LIB_DIR");
    let want_wamr = std::env::var_os("CARGO_FEATURE_WAMR_LINK").is_some();
    match std::env::var("IWASM_LIB_DIR") {
        Ok(dir) if want_wamr => {
            println!("cargo:rustc-link-search=native={dir}");
            // WAMR Windows 静态库的系统依赖（nlu-bench CI 实测）
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
                println!("cargo:rustc-link-lib=bcrypt");
                println!("cargo:rustc-link-lib=pathcch");
                if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu") {
                    println!("cargo:rustc-link-lib=winpthread");
                }
            }
            println!("cargo:rustc-link-lib=static=iwasm");
        }
        Err(_) if want_wamr => {
            panic!("wamr-link feature 需要 IWASM_LIB_DIR 指向 libiwasm.a 所在目录")
        }
        _ => {
            println!(
                "cargo:warning=loomooms-wamr: IWASM_LIB_DIR 未设置——stub 模式（Host ABI 可用，执行返回 NotLinked）"
            );
        }
    }
}
