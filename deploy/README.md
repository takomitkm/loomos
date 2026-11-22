# deploy/ — 板上部署工件

> ⚠️ **全部未上板验证**（architecture/09 §C：每个依赖完成 Host→Cross→Hardware 后才锁定）。
> 本目录是 C 区缺口的交付物：交叉构建 → 安装 → systemd 非 root → 看门狗 DT。
> 每步执行后回填 docs/dev/EVIDENCE.md。

## 文件

| 文件 | 用途 | 验证状态 |
|---|---|---|
| `loomosd.service` | systemd 非 root 单元（B4；WatchdogSec 注释待 DT） | ❌ 未验证 |
| `fmql30tai-watchdog.fragment.dts` | 看门狗 DT 片段示例（B5，需按厂商 DT 核对 compatible） | ❌ 未验证 |
| `install-board.sh` | 板上安装：用户/设备节点权限/单元安装/烟测 | ❌ 未验证 |
| `../scripts/release-aarch64.sh` | aarch64 交叉构建 + 打包 | ✅ 宿主可跑（构建产物未上板） |

## 上板流程（设计稿）

```bash
# 1. 宿主交叉构建
bash scripts/release-aarch64.sh          # 产出 dist/loomos-aarch64.tar.gz

# 2. 上传（旧板 192.168.1.113，跨网段限制见旧仓库文档）
scp dist/loomos-aarch64.tar.gz root@192.168.1.113:/tmp/

# 3. 板上安装（含 B4 非 root 化：用户/设备节点/单元）
bash /tmp/install-board.sh

# 4. 烟测（不碰 NPU 挂死类 L2/L3 项目）
systemctl start loomosd
curl http://127.0.0.1:10900/health
curl -X POST http://127.0.0.1:10900/handle_text -d '{"text":"小克，打开客厅的灯"}'

# 5. 记录（EVIDENCE.md）：RSS / 启动时间 / 端口监听 / 语音链 smoke
```

## 验收纪律引用

- 每项改动：威胁模型、测试、回退方案、目标板实测（architecture/09 §I）
- "配置项已启用" ≠ "功能已运行"（11 §11.3.2）——WatchdogSec/MemoryMax 写了不算，
  要有运行时证据
- L2/L3 板测（NPU/DMA）必须人工窗口，不进无人值守 CI
- 2026-11-20：真引擎适配器 loomos-adapters-sherpa 落地（sherpa-onnx Python API 子进程路线，宿主实跑证据见 EVIDENCE）。

---

## Nix 过渡路径（2026-11-22，ADR-012/013）

本目录的 tarball+脚本安装（install-board.sh + loomosd.service）是**过渡期兜底**：
在 Nix 镜像路径通过验收门（QEMU 可引导 + 健康检查 + 回滚演练，见
architecture/16）之前保留，之后退役。**过渡期不宣称包管理权威已替换。**

新路径（已交付，Unverified 待 CI）：

```bash
nix build .#loomos-host          # Native Host（构建即跑全仓测试，Linux）
nix build .#loomos-wamr          # libiwasm.a
nix build .#loomos-components    # wasm32-wasip1 组件
nixos-rebuild build --flake .#fmql30tai676h   # 过渡期系统配置（可评估）
```

- 可变数据：systemd StateDirectory=/var/lib/loomos（NixOS 模块 services.loomos）；
- 板端 1GB RAM：构建在开发机/CI，设备只收闭包/镜像；
- 厂商 BSP（内核 5.4.52/NPU Runtime/U-Boot）为 ExternallyProvisioned 工件，
  登记与哈希纪律见 nix/vendor/sources.nix。
