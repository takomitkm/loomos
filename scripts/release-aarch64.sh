#!/usr/bin/env bash
# aarch64 交叉构建 + 打包（宿主机执行；产物未上板验证）
# 依赖：rustup target add aarch64-unknown-linux-gnu（check 已过，见 EVIDENCE.md）
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=aarch64-unknown-linux-gnu
DIST=dist
mkdir -p "$DIST"

rustup target add "$TARGET" 2>/dev/null || true

echo "== cargo build --release --target $TARGET =="
cargo build --release --target "$TARGET" -p loomosd -p loomos-cli

STAGE="$DIST/loomos-aarch64"
rm -rf "$STAGE" "$STAGE.tar.gz"
mkdir -p "$STAGE/bin"
cp "target/$TARGET/release/loomosd" "$STAGE/bin/"
cp "target/$TARGET/release/loomos-cli" "$STAGE/bin/"
tar -czf "$STAGE.tar.gz" -C "$DIST" "$(basename "$STAGE")"

echo "== 产物 =="
ls -lh "$STAGE.tar.gz"
echo "校验："
sha256sum "$STAGE.tar.gz"
echo "⚠️ 产物未上板验证；上板流程见 deploy/README.md"
