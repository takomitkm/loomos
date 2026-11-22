//! 构建脚本：`link` feature 启用时定位 libiwasm.a（IWASM_LIB_DIR）。
//! fail-fast：feature 开但目录缺失 → cargo:error 明确报错（不再自然落入
//! 「undefined symbols」的难读链接失败）。

fn main() {
    println!("cargo:rerun-if-env-changed=IWASM_LIB_DIR");
    let link_enabled = std::env::var_os("CARGO_FEATURE_LINK").is_some();
    if !link_enabled {
        println!(
            "cargo:warning=loomos-wamr-sys: link feature 未启用——stub 模式（调用返回 NotLinked）"
        );
        return;
    }
    match std::env::var("IWASM_LIB_DIR") {
        Ok(dir) if !dir.is_empty() => {
            println!("cargo:rustc-link-search=native={dir}");
            println!("cargo:rustc-link-lib=static=iwasm");
        }
        _ => {
            println!(
                "cargo:error=loomos-wamr-sys: link feature 已启用但 IWASM_LIB_DIR 未设置（需指向含 libiwasm.a 的目录；nix build .#loomos-wamr 可产出）"
            );
        }
    }
}
