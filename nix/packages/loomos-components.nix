# Wasm 组件包：wasm32-wasip1 构建的 cdylib 组件。
# 当前组件：loomos-nlu-wasm-wrap（NLU C ABI 胶水，业务代码零改动——ADR-011 迁移规则）。
# .wasm 是正本分发产物；.aot 由部署层用 loomos-wamrc 按目标平台生成（ADR-011 §决定 7）。
{ lib, loomos-rust-platform }:
loomos-rust-platform.buildRustPackage {
  pname = "loomos-components";
  version = "0.1.0";

  src = lib.cleanSource ../..;
  cargoLock.lockFile = ../../Cargo.lock;

  # 显式钉死 cargo 输出目录为工作目录的 target/——nixpkgs 的构建环境可能预设
  # CARGO_TARGET_DIR 指向 Nix Store 路径（此前 installPhase 在 /build 下
  # 找不到 .wasm 实证）。NiX 层指定覆盖环境值，产物位置确定可定位。
  CARGO_TARGET_DIR = "target";

  # 只构建组件 crate（workspace 其余成员是 Native Host / 基准，另行打包）
  buildPhaseCargoCommand =
    "cargo build --release -p loomos-nlu-wasm-wrap --target wasm32-wasip1";
  installPhase = ''
    runHook preInstall
    mkdir -p $out/components
    # 产物位置由上方 CARGO_TARGET_DIR=target 钉死（./target 相对工作目录）。
    # 纪律：NiX 双引号字符串里美元符号加花括号是 NiX 插值（裸写报
    # undefined variable，反斜杠转义非法直接语法错误）——本 installPhase
    # 不引用任何 shell 变量。
    found=$(find target -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find /build -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件"; find target /build -name '*.wasm' -type f 2>/dev/null | head -5; exit 1; }
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
