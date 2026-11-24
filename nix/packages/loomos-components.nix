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
    # 产物位置由 nixpkgs 构建钩子设置的 CARGO_TARGET_DIR 环境变量决定
    # （构建钩子预设该值，产物不在 ./target 下，多轮实证）。
    # 关键语法：NiX 双引号字符串里插值只由 "美元加花括号" 触发——
    # 写 "${VAR}" 触发插值报 undefined variable，反斜杠转义非法报
    # 语法错误；**不带花括号的 "$VAR" 原样传给 shell**，由 shell 展开。
    # 这正是之前几轮反复报错、从未命中正确写法的坑。
    # 多路径回退：优先钩子预设目录，回退工作目录 target 与 /build。
    found=$(find "$CARGO_TARGET_DIR" -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find target -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || found=$(find /build -name '*.wasm' -type f 2>/dev/null | head -1)
    [ -n "$found" ] || { echo "未产出 .wasm 组件（CARGO_TARGET_DIR=$CARGO_TARGET_DIR）"; find target /build -name '*.wasm' -type f 2>/dev/null | head -5; exit 1; }
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
