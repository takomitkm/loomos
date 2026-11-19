#!/usr/bin/env bash
# loomos 板上安装脚本（B4 非 root 化；03-OS代劳清单.md §B4）
# ⚠️ 未上板验证。在板（Ubuntu 20.04 aarch64）以 root 执行。
set -euo pipefail

TARBALL="${1:?用法: install-board.sh /tmp/loomos-aarch64.tar.gz}"
PREFIX=/opt/loomos
DATA=/var/lib/loomos

echo "== 1. 解包 =="
mkdir -p "$PREFIX/bin" "$DATA/speech"
tar -xzf "$TARBALL" -C "$PREFIX" --strip-components=1
chmod 0755 "$PREFIX/bin/loomosd" "$PREFIX/bin/loomos-cli"

echo "== 2. 非 root 服务身份（B4）=="
id loomos >/dev/null 2>&1 || useradd --system --home "$DATA" --shell /usr/sbin/nologin loomos
groupadd -f loomos-dev
usermod -aG loomos-dev loomos
# 设备节点组权限（不依赖 device cgroup——本板未编译，只挡普通文件层访问）
for dev in /dev/udmabuf0 /dev/video0 /dev/dri/*; do
    [[ -e "$dev" ]] && chgrp loomos-dev "$dev" && chmod g+rw "$dev" || true
done
chown -R loomos:loomos "$DATA"

echo "== 3. systemd 单元 =="
install -m 0644 "$(dirname "$0")/../loomosd.service" /etc/systemd/system/loomosd.service
systemctl daemon-reload

echo "== 4. 烟测（root 预检，非 root 正式验证走 systemctl）=="
sudo -u loomos "$PREFIX/bin/loomos-cli" --version 2>/dev/null || "$PREFIX/bin/loomos-cli" --version

echo "== 完成。启动: systemctl start loomosd；验收命令见 deploy/README.md =="
echo "== 记住：本脚本未上板验证；执行后把读数回填 docs/dev/EVIDENCE.md =="
