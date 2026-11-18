这里是用户文档，记录接口，调度入口，I/O等等


---

## normos 原型用户面（2026-11-19 并入）

### 用户文档规则

1. **两层分离**：`docs/user/` 给「跑系统的人」（安装/配置/命令/故障排查）；
   `docs/dev/` 给「改系统的人」（架构/证据/任务）。不混写。
2. 命令必须可复制执行：每个命令块都要能在干净环境直接粘贴成功；写文档前先跑一遍。
3. 不写没有的开关：只记录实际存在的配置与端点；计划中的功能写「roadmap」。
4. 错误信息可检索：用户可能看到的报错文案，必须能在仓库里 grep 到出处。
5. 板/宿主分开：性能与兼容结论必须标明出处环境；不给「应该也能跑」式承诺。
6. 中文优先，术语首次出现给英文对照；引用架构约束时链接原文，不转述拷贝。

### 当前接口（原型，默认档全 mock 全自检）

- 启动：`cargo run -p normos-cli -- serve`（控制台 http://127.0.0.1:10900）
- 文本执行：`POST /handle_text {"text":"小克，打开客厅的灯"}` → ExecReport（意图/动作/回执）
- 直接动作：`POST /action {"intent":"HassTurnOn","slots":{"entity_id":"light.keting"}}`
- 状态/实体/事件：`GET /status`、`GET /entities`、`GET /events?after=0`、`GET /health`
- 配置：`GET /config/{namespace}`（Schema/Revision 见 docs/dev/architecture/04）
- 一键验收：`bash agent/run-prototype.sh`（退出码即验收结论）
- I/O：语音夹具 `fixtures/*.wav`（48 kHz）；播报落盘 `reports/speech/*.wav`（48 kHz，
  沿旧板「无声卡→落盘」纪律）；事件 `reports/data/events.jsonl`
