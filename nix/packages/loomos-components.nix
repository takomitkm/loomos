# Wasm 组件包：wasm32-wasip1 构建的 cdylib 组件。
# 当前组件：loomos-nlu-wasm-wrap（NLU C ABI 胶水，业务代码零改动——ADR-011 迁移规则）。
# .wasm 是正本分发产物；.aot 由部署层用 loomos-wamrc 按目标平台生成（ADR-011 §决定 7）。
{ lib, loomos-rust-platform }:
loomos-rust-platform.buildRustPackage {
  pname = "loomos-components";
  version = "0.1.0";

  src = lib.cleanSource ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  # 只构建组件 crate（workspace 其余成员是 Native Host / 基准，另行打包）
  buildPhaseCargoCommand =
    "cargo build --release -p loomos-nlu-wasm-wrap --target wasm32-wasip1";
  installPhase = ''
    runHook preInstall
    mkdir -p $out/components
    # 语法要点（多轮 CI 实证）：
    # - NiX 缩进字符串整体插值：美元符加花括号在求值期被展开，注释内的写法
    #   也不例外（实测报 undefined variable）；反斜杠转义美元符在本版本不
    #   可靠（实测 syntax error）——故本块彻底不出现「美元符加花括号」组合
    # - 无花括号的美元变量原样传给 shell 展开；但 cargo 钩子在 buildPhase 内
    #   覆写目标目录且其 export 不跨 phase，installPhase 无法预知产物落点
    # 策略：构建沙箱内唯一 loomos_nlu_wasm_wrap.wasm 即正本——跨挂载点全盘
    # 定位；剪除 /nix（store 只读，产物必在沙箱根文件系统）与伪文件系统
    # /proc /sys /dev /run（避免误扫与耗时）
    found=$(find / \( -path /nix -o -path /proc -o -path /sys -o -path /dev -o -path /run \) -prune -o -name 'loomos_nlu_wasm_wrap.wasm' -type f -print 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件（全盘搜索 loomos_nlu_wasm_wrap.wasm 无果）"; find / \( -path /nix -o -path /proc -o -path /sys -o -path /dev -o -path /run \) -prune -o -name '*.wasm' -type f -print 2>/dev/null | head; exit 1; }
    cp "$found" $out/components/
    runHook postInstall
  '';

  # 组件行为等价由宿主侧基准（loomos-nlu-bench）与 CI QEMU 验证，不在本包内跑 wasm 单测
  doCheck = false;

  meta = with lib; {
    description = "loomos Wasm 组件产物（wasm32-wasip1，.wasm 正本）";
    license = licenses.lgpl3Only;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}
