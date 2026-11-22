# 厂商/外部工件登记正本（ADR-012 §决定 5）。
#
# 分类（必填 classification）：
#   SourceReproducible     —— 可从源码重建（Nix 直接构建）
#   ArtifactReconstructible—— 由钉死工件 + 哈希重建
#   ExternallyProvisioned  —— 授权用户外部获取，重建过程只做哈希校验
#   Unavailable            —— 当前缺失
#
# 哈希纪律：hash 字段只有拿到真实工件并计算 sha256 后才能填值；
# 禁止占位哈希、禁止虚构"可复现编译"。凭证/私钥/可变运行数据不进本文件。
{ lib }:
{
  # ---- FMQL30TAI676H（悟净）板端 BSP（docs/dev/platforms/FMQL30TAI676H/）----
  fmql30tai676h = {
    vendor-kernel-5.4.52 = {
      classification = "ExternallyProvisioned";
      version = "5.4.52（Ubuntu 20.04.4 BSP 补丁内核）";
      source = "厂商 FMQL SDK / 板固件配套";
      license = "GPL-2.0（内核）+ 厂商补丁（授权渠道）";
      arch-abi = "aarch64-linux-gnu";
      hash = null; # 未提供：拿到工件后计算 sha256 填入
      obtain = "厂商授权渠道；获取与登记步骤见 docs/dev/platforms/FMQL30TAI676H/";
      runtime-deps = [ ];
      note = "过渡期系统配置先用标准 aarch64 内核（可评估/可 QEMU）；上板替换 boot.kernelPackages 时才需要本工件";
    };
    vendor-npu-runtime = {
      classification = "ExternallyProvisioned";
      version = "未知（随固件分发）";
      source = "厂商 FMQL SDK";
      license = "厂商专有";
      arch-abi = "aarch64-linux-gnu";
      hash = null;
      obtain = "厂商授权渠道";
      runtime-deps = [ ];
      note = "Native Host 推理后端；Wasm Guest 永不直接取得其指针（ADR-011 v1.1 主权边界）";
    };
    bootloader-uboot-2022.10 = {
      classification = "ExternallyProvisioned";
      version = "2022.10 + FSBL + BL31";
      source = "厂商 FMQL SDK";
      license = "混合（U-Boot GPL-2.0 + 厂商闭源阶段）";
      arch-abi = "aarch64-linux-gnu";
      hash = null;
      obtain = "厂商授权渠道；不重建启动链（ADR-013 证据边界）";
      runtime-deps = [ ];
    };
  };

  # ---- 通用推理后端 ----
  onnxruntime = {
    classification = "ArtifactReconstructible";
    version = "v20（OrtApi 307 槽位，与 loomos-ort-sys gen.rs 对齐）";
    source = "https://github.com/microsoft/onnxruntime（v20 系列 release）";
    license = "MIT";
    arch-abi = "x86_64-linux-gnu / aarch64-linux-gnu / windows-x86_64";
    hash = null; # 钉死 release 后计算
    obtain = "官方 release 工件或 nixpkgs onnxruntime 构建接入（P3 决定）";
    runtime-deps = [ ];
  };
  sherpa-onnx = {
    classification = "SourceReproducible";
    version = "2.0.2";
    source = "https://github.com/k2-fsa/sherpa-onnx（官方源码可重建；Python wheel 子进程路线已实测）";
    license = "Apache-2.0";
    arch-abi = "宿主机运行（平台无关子进程协议）";
    hash = null;
    obtain = "Nix 构建接入（P3）；过渡期按 loomos-adapters-sherpa 现行 wheel 路线";
    runtime-deps = [ "onnxruntime" ];
  };
  silero-vad-model = {
    classification = "ArtifactReconstructible";
    version = "1.0（仓库根 silero_vad.onnx，随仓库分发）";
    source = "https://github.com/snakers4/silero-vad";
    license = "MIT";
    arch-abi = "平台无关（模型文件）";
    hash = null; # 待办：对仓库内工件计算 sha256 登记
    obtain = "仓库内自带；升级模型版本时更新哈希";
    runtime-deps = [ ];
  };
}
