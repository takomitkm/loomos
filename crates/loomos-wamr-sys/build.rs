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
            // Windows：WAMR win_file.c 引用 PathCchCombine。
            // - msvc 工具链：链接系统导入库 shlwapi.lib（标准路径）；
            // - windows-gnu（rustup self-contained 无 shlwapi 导入库——dlltool 缺链家族问题）：
            //   本机验证走 RUSTFLAGS 直接喂 DLL：
            //   RUSTFLAGS='-C link-arg=C:\Windows\System32\shlwapi.dll'
            //   （GNU ld 可直接以 PE DLL 为输入；CI Linux 不需要此处理）
            if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
                && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
            {
                println!("cargo:rustc-link-lib=dylib=shlwapi");
            }
        }
        _ => {
            println!(
                "cargo:error=loomos-wamr-sys: link feature 已启用但 IWASM_LIB_DIR 未设置（需指向含 libiwasm.a 的目录；nix build .#loomos-wamr 可产出）"
            );
        }
    }
}
