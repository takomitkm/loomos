# vendor-sdk：ExternallyProvisioned 厂商工件的校验入口。
# 工件未提供时 throw 明细（获取方式见 nix/vendor/sources.nix 各条目 obtain 字段）。
# 提供后：requireFile 用登记的 sha256 校验——重建过程只校验，不虚构编译。
{ lib, sources ? import ../vendor/sources.nix { inherit lib; }, requireFile }:
let
  flat = lib.concatLists (lib.mapAttrsToList
    (platform: artifacts: lib.mapAttrsToList (name: meta: {
      inherit platform name meta;
    }) artifacts) sources);

  unprovided = lib.filter (e: e.meta.classification or "" == "ExternallyProvisioned"
    && (e.meta.hash or null) == null) flat;

  report = lib.concatStringsSep "\n" (map (e:
    "  - ${e.platform}/${e.name}: ${e.meta.version or "?"} | 获取: ${e.meta.obtain or "见登记表"}")
    unprovided);
in
if unprovided != [ ] then
  throw ''
    loomos vendor-sdk：以下厂商工件未提供（ExternallyProvisioned，ADR-012 §决定 5）：
    ${report}
    获取后计算 sha256 填入 nix/vendor/sources.nix 对应条目，再构建本包。
  ''
else
  # 全部已提供：逐条 requireFile 校验（P5 接入真实打包时展开）
  requireFile {
    name = "loomos-vendor-sdk-placeholder";
    url = "placeholder";
    sha256 = lib.fakeSha256;
    message = "全部厂商工件已登记——本占位将在 P5 替换为真实 requireFile 组装";
  }
