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
    # nixpkgs 的 cargo 构建钩子预设了 CARGO_TARGET_DIR（产物不在 ./target
    # 下，多轮实证），但钩子内的 export 不跨 phase 传给 installPhase。
    # 语法要点（多轮 CI 实证）：
    # - NiX 双引号字符串整体插值，美元加花括号恒被求值（注释内的写法也
    #   不例外，实测报 undefined variable）
    # - 反斜杠加美元是合法 NiX 转义，产生字面美元符号，转义后的写法在
    #   NiX 源里合法、由 shell 展开
    # - 无花括号的美元变量原样传给 shell，但未定义时 set -u 直接报错
    # 策略：cdir 取环境变量（默认空），依次回退 $out/target、target、/build。
    cdir=\${CARGO_TARGET_DIR:-}
    found=$(find "$cdir" -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find $out/target -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find target -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find /build -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件（cdir=$cdir，查找 $out/target）"; find $out/target target /build -name '*.wasm' -type f 2>/dev/null | head -5; exit 1; }
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
