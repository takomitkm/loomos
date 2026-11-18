# Agent Task: 复跑/复验 normos 原型

## Objective

把本仓库跑成一台可验收的原型：语音链（唤醒→转写→NLU→执行→播报落盘）、
运行时（租约/Capability/配置/生命周期/Supervisor）、设备框架（mock + 板端 HTTP 契约）
全部工作，并产出证据报告。

## Steps

1. `bash agent/run-prototype.sh`。退出码 0 即验收通过。
2. 若失败：读 `reports/prototype-report.md` 场景表与 `/tmp/normos-*.log`，按
   AGENTS.md §5 的任务路径修复；**不得**通过删测试/改期望让脚本变绿——
   期望变更必须给出行为依据。
3. 复跑直到全绿，然后汇报：
   - 测试数量与 smoke 结果（12/12）；
   - 各级耗时（resample/vad/asr/kws/executor/tts 的 min/P50/P95/max）；
   - 产物清单（报告 / 事件日志 / 语音 wav）；
   - 证据等级（Tested/TypeChecked）与残余假设（mock 档清单）。

## Constraints

- 默认档必须保持「空机可跑全链」（echo/sine 纪律）；真实引擎是增量适配器。
- 不引入需要 C 编译器/原生库的依赖（保持纯 Rust 依赖树）。
- 平台事实只能引用 docs/dev/platforms/ 的证据等级，不得假设能力存在。
