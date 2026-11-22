# rauc-system-conf.nix — 板端 RAUC system.conf 模板（P5 占位）。
#
# 真实 slot 布局（block device 映射）**待板端 boot 环境调查**（ADR-013 证据边界）——
# 本模板只固化 RAUC 与 U-Boot 的对接形态，不生成真实分区方案。
{ compatible ? "fmql30tai676h" }:
''
# /etc/rauc/system.conf — loomos A/B（U-Boot bootenv 对接）
[system]
compatible=${compatible}
bootloader=uboot
# U-Boot 2022.10 bootenv 变量（板端调查后确认变量名与语义）：
#   rauc.slot=A/B  bootcount / altbootcmd 语义按 U-Boot bootcount 标准接入
# mountprefix=/mnt/rauc
# statusfile=/var/lib/loomos/rauc-status

[slot.rootfs.A]
device=/dev/mmcblk?pA    # 占位：板端分区调查后替换
type=ext4
bootname=A

[slot.rootfs.B]
device=/dev/mmcblk?pB    # 占位
type=ext4
bootname=B
''
