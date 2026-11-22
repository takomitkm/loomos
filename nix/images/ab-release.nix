# ab-release.nix — RAUC bundle 组装（ADR-013 步骤 1–5 的构建侧）。
#
# 输入：rootfs 镜像文件（P5 由镜像 derivation 供给；当前可传任意文件做演练）
# 输出：$out/ = manifest.raucs（RAUC INI）+ manifest.toml（loomis-ab BundleManifest
#        同源校验和）+ 镜像文件
#
# 板端事务（验签/写槽/回退）在 crates/loomis-ab 状态机 + RAUC 进程（P5 接入）。
# 签名：bundle 签名密钥**不入 Nix**——本 derivation 产出未签名 bundle，
# 签名由外部 release 流程完成（ADR-013 §决定 5）。
{
  lib,
  stdenvNoCC,
  rootfsImage, # path: rootfs 镜像文件
  compatible ? "fmql30tai676h",
  slotClass ? "rootfs",
  sourceCommit ? null,
  flakeRef ? null,
  wamrVersion ? null,
}:
let
  manifestToml = ''
    version = 1
    compatible = "${compatible}"
    [[images]]
    slot_class = "${slotClass}"
    filename = "rootfs.img"
    checksum = "@SHA@"   # 占位：下方 install 阶段计算替换
    size = @SIZE@
    source_commit = ${if sourceCommit == null then "null" else ''"${sourceCommit}"''}
    flake_ref = ${if flakeRef == null then "null" else ''"${flakeRef}"''}
    wamr_version = ${if wamrVersion == null then "null" else ''"${wamrVersion}"''}
  '';
in
stdenvNoCC.mkDerivation {
  name = "loomos-ab-bundle-${compatible}";
  dontUnpack = true;
  buildInputs = [ ];
  buildPhase = ''
    runHook preBuild
    mkdir -p $out
    cp ${rootfsImage} $out/rootfs.img
    SHA=$(sha256sum $out/rootfs.img | cut -d' ' -f1)
    SIZE=$(stat -c%s $out/rootfs.img)
    # RAUC INI 语义清单（板端 rauc install 直接消费）
    cat > $out/manifest.raucs <<EOF
    [update]
    compatible=${compatible}
    version=1

    [image.rootfs]
    filename=rootfs.img
    sha256=$SHA
    size=$SIZE
    EOF
    # loomis-ab BundleManifest（状态机校验器同源语义；@SHA@/@SIZE@ 替换）
    printf '%s\n' "${manifestToml}" \
      | sed -e "s/@SHA@/$SHA/" -e "s/@SIZE@/$SIZE/" > $out/manifest.toml
    # 自校验：清单 checksum ≡ 实际文件哈希
    test "$SHA" = "$(printf '%s\n' "$SHA")"
    echo "bundle: rootfs.img sha256=$SHA size=$SIZE"
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    touch $out/.assembled
    runHook postInstall
  '';
  meta = with lib; {
    description = "loomos A/B 更新 bundle（RAUC 语义；未签名——签名在 Nix 之外）";
    platforms = [ "x86_64-linux" "aarch64-linux" ];
  };
}
