# ADR-042：连接级语音、听写与双后端

- 状态：Accepted。
- 日期：2026-09-25。
- 授权：实现 Paseo 语音与听写的八个接口；用户明确要求云端和本地两种后端。
- 来源：本地 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`。

## 归属与依赖

新增纵向能力包 `server-voice`，拥有协议 DTO、能力安装、dispatcher、连接状态机、STT/TTS
端口与 HTTP/本地进程 adapter。其唯一 workspace 依赖为 `server-model`。API 和 binary
增加对它的依赖；model/protocol/domain 不依赖 voice。修订 ADR-037/038 的能力包清单，
不改变 Message、Session、Run 的领域语义。依赖守卫覆盖新边。

`server-api` 仅负责请求/事件方向、协商、物理连接和轮询；请求先进入 voice dispatcher。
binary 实现 voice 定义的 `Agents` 业务端口，通过已有 `AgentExecution` 发送、等待和取消
原生 turn。voice 不依赖 provider，也不修改持久化历史、Agent 默认配置或权限模式。
provider 的私有 `internal.voice.send/status/cancel` 在同一个 worker 命令通道上使用 native
turn ID 校验所有权，迟到的取消不能打断随后提交的文字 turn，迟到的结果读取不能朗读新 turn。
发送还携带连接任务的取消令牌进入 Provider 队列：已取消的排队请求不启动 turn；发送途中
取消或回执接收者消失时，worker 负责取消刚接受的 turn，避免超过连接清理期限后遗留工作。
这些内部方法不在 WebSocket catalog，也不添加公开 capability。

## 协议和生命周期

八个规范方法沿用 ADR-026：`voice.mode.set.request`、`voice.abort.request` 使用 request；
`voice.audio.chunk`、`voice.audio.played` 和四个 `dictation.stream.*` 使用 event。
业务参数在 params，字段保持 Paseo camelCase；requestId 只使用外层 request_id。
模式切换结果在 response.result，其他结果使用规范事件，完整字段与例子见
[操作说明](../operations/server-voice.md)。旧下划线名称不作为 wire alias。

每个物理 socket 独占自己的听写、音频输入、模式和播放确认；同 client_id 的重连也不能
继承状态。每个 Agent 同时最多由一个语音连接占用；任务持有目标租约直到取消收尾完成。
转写不自动创建 Agent；只有已启用模式的语音输入才提交原生文本 turn。

听写按 seq 进行有界乱序重排、重复检测和连续 ack；重复 start 保留状态，finish 等待所有
分片直到 finalSeq，不将缺失音频当成成功。成功 final 在连接内缓存 60 秒，重复 finish
重放结果且不重复调用模型。cancel、空闲超时、断开和 shutdown 取消后台任务；generation
标识拒绝迟到结果污染同名新流。partial 从累计 PCM 快照计算，失败不删除已确认音频。

语音输入支持显式 isLast，PCM 还支持振幅门限和 600ms 静音结束。新的说话开始立即取消
旧的 STT、Agent 等待、TTS 和待播放数据；替换 turn 等待同目标之前的取消收尾。
输出使用独立 ID 与 groupId/chunkIndex/isLastChunk；至多四片未经播放确认的音频，
超过 30 秒未确认释放播放状态并报错。

每连接最多四个活动听写，全进程最多十六个，推理并发四个；单流音频上限 16MiB，
单片 512KiB，seq 小于 16384，乱序窗口 128 片。finish 总预算 120 秒，未结束流空闲预算
60 秒。任务纳入公共 TaskTracker；适配器收到取消后显式终止并等待本地子进程。

## 双后端与差异

STT 和 TTS 可分别选择 `openai`、`local` 或 `disabled`，默认禁用，不因存在其他功能的
OPENAI_API_KEY 自动启用上传。显式选择 openai 后可从环境变量引用凭据，自定义 endpoint
支持兼容服务。HTTP 禁止重定向，不返回服务响应体或请求 URL 中的内部错误。

本地实现调用 whisper.cpp CLI 与 Piper CLI，使用用户已安装的模型，无 shell 字符串拼接，
无自动下载；临时音频只存在于私有临时目录，完成/取消后清除。这里没有移植 Paseo 的
Sherpa/Parakeet Node worker、模型目录/下载器或神经 VAD。HTTP 使用有界文件转写，partial
是快照重识别；TTS 合成完成后按 PCM 分片播放，不承诺 provider 原生实时流延迟。

两种后端共享 mono PCM16 / PCM16 WAV 输入，支持 8/16/22.05/24/44.1/48kHz。压缩容器
不隐式猜测、解码或安装转码器；客户端必须传受支持音频格式。whisper 输入重采样为16kHz。

## 验证

测试覆盖连接协议、乱序/重复/取消、模式与目标排他、播放确认、真实 HTTP multipart/PCM
调用，以及本地可执行程序的参数、输入输出和取消回收。外部依赖使用离线替身；真实模型
的识别质量和延迟仍需另行验收。命令、覆盖率与限制见[实施报告](../reports/server-voice.md)。
