# 语音、听写与双后端实现

实现 [ADR-042](../decisions/adr-042-server-voice.md)：八个 Paseo 方法已从占位处理接入
独立 `server-voice` 能力包，并在生产 server 装配。固定参考 Paseo
`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。

| 方法 | 行为 |
| --- | --- |
| voice.mode.set.request | 连接启用/关闭语音、解析目标 Agent、独占目标租约 |
| voice.abort.request | 取消输入、识别、Agent 等待、合成和待播放输出 |
| voice.audio.chunk | PCM/WAV 输入、显式结束或静音分句、插话取消旧任务 |
| voice.audio.played | 按音频 ID 确认播放，限制未确认输出并处理超时 |
| dictation.stream.start | 校验后端和格式、连接内流创建及幂等重发 |
| dictation.stream.chunk | 有界乱序重排、重复检测、连续序号确认和 partial |
| dictation.stream.finish | 等待缺片、最终转写、结果缓存与重复 finish 重放 |
| dictation.stream.cancel | 取消后台识别并丢弃该流，防止迟到结果污染重建流 |

生产已实现 **155/195** 个方法；Paseo 规范方法为 **148/188（78.72%）**，比 ADR-041
增加 8 个，剩余占位 **40** 个。方法接通不表示所有音频格式、模型和延迟均与 Paseo 等价。

## 后端与边界

识别和合成可分别选择 OpenAI 兼容 HTTP、本地或禁用。本地识别调用 whisper.cpp，
本地合成调用 Piper，需事先安装程序和模型；云端凭据从环境注入。默认禁用，显式配置后
才调用后端。完整变量、消息字段与示例见[操作说明](../operations/server-voice.md)。

voice 仅依赖 server-model；binary 实现 voice 的 Agent 端口，复用已有原生文本执行。
Provider 校验 native turn ID，避免旧语音任务取消或读取后续文本 turn。取消令牌同时
进入 Provider 队列，排队期间取消不发请求，发送途中取消及回执丢失由 worker 收尾。
没有改变 Message 历史、Session 指针或领域依赖边界。

每个物理连接独立持有流和播放状态；断线及 shutdown 取消任务。音频、乱序窗口、流数量、
模型并发、等待时间及播放确认都有上限。partial 是累计 PCM 快照重识别；TTS 完成后分片
播放。当前没有 Sherpa/Parakeet、自动模型下载、神经 VAD 或压缩音频解码。

## 验证

测试包括有界重排、重复与冲突、缺片/空闲/推理/播放超时、取消与迟到结果、目标排他、
播放背压、实际 localhost HTTP multipart 和 PCM、Unicode 长文本合成分段、本地 CLI
参数及重采样、子进程终止/回收和私有临时文件清理。WebSocket 测试校验方向、协商和物理
连接隔离；生产进程测试贯通语音 HTTP 替身、原生 Agent 替身和合成输出，并验证断线取消。
另有延迟发送回执测试，验证取消不会遗留 turn 或打断后来提交的 turn。

最终普通完整测试：**975 passed、0 failed、5 ignored**，
99 个目标，包含 1 项 doctest。fmt、完整 workspace Clippy `-D warnings` 和
`git diff --check` 通过。中途并行验证曾因进程启动/CLI 超时中止，随后使用最终源码串行
完整重跑；中止结果不计入最终通过数或覆盖率。

## Test coverage

工作区行覆盖率 **82.74%（45,938/55,523）**。
可比基线为 [ADR-041 制品](server-agent-controls-coverage.json)，同平台、默认 features 和
统计范围。覆盖率与测试通过数分别统计。

| 范围 | 覆盖行/总行 | 行覆盖率 | 相对 ADR-041 |
| --- | ---: | ---: | ---: |
| workspace | 45,938/55,523 | 82.74% | +0.28 个百分点 |
| server-voice | 1,192/1,275 | 93.49% | 新增 crate，无历史基线 |
| server-provider | 6,184/6,712 | 92.13% | +0.03 个百分点 |
| server-api | 867/892 | 97.20% | +0.17 个百分点 |
| server-bin | 463/483 | 95.86% | -0.17 个百分点 |

[可审阅 JSON 制品](server-voice-coverage.json)包含聚合数据、逐文件行数和源码哈希、
工具链、精确命令、基线、跳过测试原因及验证日志哈希。
覆盖率完整运行：**974 passed、0 failed、5 ignored**，
73 个目标；cargo-llvm-cov 默认不测 doctest。五项 ignored 是两个真实 Codex 场景、
两个真实 DeepSeek 场景和一个要求外置 worker 的重放场景。外部服务全部使用可控离线替身，
未调用付费 API、真实账户或已训练的语音模型。

测量基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba` 的工作树，保留已有 ADR-038/039/040/041 修改并加入本轮 ADR-042。
686 个 Rust/Python/manifest 源文件 SHA-256：

`bf6565b772e311c145ab2f5cfdf27c0a0bcec01071e008ffa0f45e33998e767e`

开始和结束的源码快照一致。范围为 macOS arm64 全工作区、默认 features、默认 source
filters，无显式文件排除；Linux/Windows 未测。最终覆盖率运行使用默认清理，未合并前次
中止运行的 profile。精确命令：

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j2
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-voice-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --show-missing-lines --offline
```

本机 HTML：`/tmp/ait-agent-interface-target/llvm-cov/html/index.html`；仓库内 JSON 为可审阅制品，
不依赖本机 HTML 路径。

尚未覆盖的分支包括：32 条完成缓存满载时的淘汰、全部本地/云端/禁用配置组合，以及部分
Agent 归档、发送拒绝和执行失败路径。其他未充分验证的行为包括真实语音识别/合成质量
与延迟、真实认证服务及
代理/网络异常、全部畸形 WAV
组合和最大音频尺寸耗尽、本地进程 kill/reap 的系统级失败。后续需在配置好的模型与麦克风
上进行显式启用的验收，并验证 Linux/Windows 的 CLI 安装与兼容性。这些限制不计作已验收。
