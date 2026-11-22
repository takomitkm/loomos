# WAMR 运行时库：libiwasm.a（静态）+ iwasm CLI。
# 源码钉死（flake input wamr-src = main@06f6c8c6e64e75eee6b8a3ce06ac2c7d10d015bf）。
# feature 最小集（ADR-011 §4.5）：AOT + Interpreter + WASI P1；无 JIT/线程/多模块/SIMD。
# 产物名注意：vmlib target 的 OUTPUT_NAME=iwasm → libiwasm.a（与本机/QEMU 实测一致）。
{ lib, stdenv, cmake, wamrSrc }:
stdenv.mkDerivation {
  pname = "loomos-wamr";
  version = "2026.11.20"; # wamr-src 钉死 commit 的日期（升级时同步改）

  src = wamrSrc;
  # WAMR build-scripts/version.cmake 会写源码树（core/version.h，位于 sourceRoot 之外）——
  # postPatch（sourceRoot 内）覆盖不到，postUnpack 放开整树（CI 实证 Permission denied）
  postUnpack = ''
    chmod -R u+w source
  '';
  nativeBuildInputs = [ cmake ];
  # 官方 Linux 构建入口（product-mini/platforms/linux）
  sourceRoot = "source/product-mini/platforms/linux";

  cmakeFlags = [
    "-DWAMR_BUILD_PLATFORM=linux"
    "-DWAMR_BUILD_AOT=1"          # 生产执行模式
    "-DWAMR_BUILD_INTERP=1"       # 开发后备
    "-DWAMR_BUILD_FAST_JIT=0"     # 嵌入式目标不用 JIT
    "-DWAMR_BUILD_LLVM_JIT=0"
    "-DWAMR_BUILD_WASI=1"
    "-DWAMR_BUILD_LIBC_WASI=1"    # WASI Preview 1（ADR-011 Host ABI 基座）
    "-DWAMR_BUILD_LIB_PTHREAD=0"
    "-DWAMR_BUILD_SIMD=0"
    "-DWAMR_BUILD_MULTI_MODULE=0"
  ];

  # 产物位置随 WAMR CMake 布局变化，find 兜底
  installPhase = ''
    runHook preInstall
    mkdir -p $out/lib $out/bin
    found_lib=$(find . -name 'libiwasm.a' -type f | head -1)
    [ -n "$found_lib" ] || { echo "libiwasm.a 未产出"; exit 1; }
    cp "$found_lib" $out/lib/libiwasm.a
    found_bin=$(find . -name 'iwasm' -type f | head -1)
    [ -n "$found_bin" ] && cp "$found_bin" $out/bin/iwasm || echo "iwasm CLI 未产出（仅库）"
    runHook postInstall
  '';

  meta = with lib; {
    description = "WAMR 运行时（loomos 钉死构建）：AOT+Interpreter+WASI P1";
    license = licenses.asl20;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}
