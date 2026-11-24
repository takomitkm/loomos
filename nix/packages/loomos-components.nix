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
    # Nix 的 cargo 输出目录由 CARGO_TARGET_DIR 决定；构建在 /build 下进行。
    # 宽范围搜索（CARGO_TARGET_DIR + target + /build），失败时打印诊断。
    # 注意：Nix 构建 shell 开 set -u，未定义变量必须默认展开（否则报
    # 'undefined variable'）。
    if [ -n "${CARGO_TARGET_DIR:-}" ]; then
      found=$(find "$CARGO_TARGET_DIR" -name '*.wasm' -type f 2>/dev/null | head -1)
    fi
    [ -n "$found" ] || found=$(find /build -name '*.wasm' -type f 2>/dev/null | head -1)
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
