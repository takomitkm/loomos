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
    # Nix 的 cargo 输出目录由 CARGO_TARGET_DIR 决定（可能指向 Nix Store 路径，
    # 不在 /build 内——此前 find /build 未命中实证）。
    # 关键：NiX '' 字符串中 ${...} 是 NiX 插值，不是 shell 变量——
    #   - 写 ${CARGO_TARGET_DIR} → NiX 报 'undefined variable'
    #   - 写 ${CARGO_TARGET_DIR:-} → NiX 语法错误
    # 必须转义为 \$CARGO_TARGET_DIR 让 shell 拿到字面量，并给默认值回退 /build。
    found=$(find "\${CARGO_TARGET_DIR:-/build}" -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find target -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件"; find /build -name '*.wasm' -type f 2>/dev/null | head -5; exit 1; }
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
