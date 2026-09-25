# 独立 server 的语音与听写

八个方法为实际实现；后端默认禁用。先在 server 进程环境中选择后端，再启动 server。
未配置 STT/TTS 时返回 `speech_backend_unavailable`。

## 后端配置

OpenAI 兼容服务：

```sh
export AIT_SPEECH_PROVIDER=openai
export AIT_SPEECH_BASE_URL=https://api.openai.com/v1
export AIT_SPEECH_STT_MODEL=whisper-1
export AIT_SPEECH_TTS_MODEL=tts-1
export AIT_SPEECH_VOICE=alloy
# 通过运行环境注入 AIT_SPEECH_API_KEY；也支持 OPENAI_API_KEY 回退。
```

本地离线：

```sh
export AIT_SPEECH_PROVIDER=local
export AIT_SPEECH_WHISPER_BIN=/absolute/path/to/whisper-cli
export AIT_SPEECH_WHISPER_MODEL=/absolute/path/to/ggml-model.bin
export AIT_SPEECH_PIPER_BIN=/absolute/path/to/piper
export AIT_SPEECH_PIPER_MODEL=/absolute/path/to/voice.onnx
```

Piper 需要模型配套的 `voice.onnx.json` 和匹配语言的 voice；whisper 模型须支持录音语言。
程序、模型由用户安装；server 不安装依赖、不下载模型。可用 `AIT_SPEECH_STT_PROVIDER`
和 `AIT_SPEECH_TTS_PROVIDER` 分别覆盖公共选择，例如本地识别、云端合成，或只启用听写。
`disabled` 关闭对应能力。更改环境配置后重启 server；密钥不进入 config.json 或日志。

云端实现依据 [OpenAI 文件转写](https://developers.openai.com/api/docs/guides/speech-to-text)
和 [TTS](https://developers.openai.com/api/docs/guides/text-to-speech) 的 HTTP 协议；本地参数依据
[whisper.cpp CLI](https://github.com/ggml-org/whisper.cpp/blob/master/examples/cli/README.md)
和 [Piper CLI](https://github.com/OHF-Voice/piper1-gpl/blob/main/docs/CLI.md)。

## 消息

在 `/v1/ws` 的 hello 中协商需要的方法。所有音频为 Base64；推荐格式
`audio/pcm;rate=16000;bits=16;channels=1`，也支持完整 `audio/wav`（单声道 PCM16）。
PCM 支持 8000、16000、22050、24000、44100、48000Hz。压缩容器不支持。

| 方法 | 信封 | params |
| --- | --- | --- |
| voice.mode.set.request | request | enabled、agentId（启用时必填） |
| voice.abort.request | request | 空对象 |
| voice.audio.chunk | event | audio、format、isLast |
| voice.audio.played | event | id |
| dictation.stream.start | event | dictationId、format |
| dictation.stream.chunk | event | dictationId、seq、audio、format |
| dictation.stream.finish | event | dictationId、finalSeq |
| dictation.stream.cancel | event | dictationId |

听写流程：start → ack(-1) → chunk(seq 从0开始) → ack(最高连续序号) → finish →
finish.accepted → final。乱序包最多领先128片，相同包重发幂等；同序号不同内容报错。
finalSeq 为最后一片的序号，不是分片数量。finish 之后可补发缺片，120秒内仍缺片则报错。
重复 finish 在结果缓存存活期间返回同一个 final，不再次计费。cancel 幂等，无成功事件。

```json
{"type":"event","method":"dictation.stream.start","params":{"dictationId":"d1","format":"audio/pcm;rate=16000;bits=16"}}
{"type":"event","method":"dictation.stream.chunk","params":{"dictationId":"d1","seq":0,"audio":"AAABAA==","format":"audio/pcm;rate=16000;bits=16"}}
{"type":"event","method":"dictation.stream.finish","params":{"dictationId":"d1","finalSeq":0}}
```

上面的短音频仅演示编码，识别结果由后端决定。

语音先以 request 启用目标 Agent。response.result 含 accepted、enabled、agentId、error，
失败时可带 reasonCode/retryable。上传音频触发转写，成功后调用现有 Agent 文本执行，
等待最终回复并合成语音。未启用模式时只发转写结果。不要把听写结果自动当成已提交的消息。

```json
{"type":"request","request_id":"mode1","method":"voice.mode.set.request","params":{"enabled":true,"agentId":"existing-agent-id"}}
```

| 服务端 event.method | params |
| --- | --- |
| voice.input.state | isSpeaking |
| voice.transcription.result | requestId、text、可选 language |
| voice.audio.output | audio、format、id、isVoiceMode、groupId、chunkIndex、isLastChunk |
| voice.error | error、reasonCode、retryable |
| dictation.stream.ack | dictationId、ackSeq |
| dictation.stream.finish.accepted | dictationId、timeoutMs |
| dictation.stream.partial | dictationId、text |
| dictation.stream.final | dictationId、text |
| dictation.stream.error | dictationId、error、reasonCode、retryable |

每片 audio.output 实际播放完后回传 voice.audio.played。至多四片待确认，30秒不确认报错。
新的 voice.input.state(isSpeaking=true) 表示插话，客户端应清除之前排队的语音；显式
abort/关闭模式也应立即停止客户端播放。生成的语音应在客户端标明为 AI 合成语音。

状态只属于当前物理连接，不持久化。重连需使用新流并重新发送录音，不能将旧 ack 当成
新连接的接收进度。partial 每至少2秒对累计 PCM 重识别，可能被后续文本修正；WAV 只发
最终结果。客户端应保存录音直到 final，并按 reasonCode/retryable 显示可恢复错误。

本地后端使用 whisper.cpp/Piper，未包含 Paseo Sherpa/Parakeet 自动模型管理；VAD 是简单
振幅门限和600ms静音判断。两类 adapter 的输出质量、延迟和语言覆盖取决于实际模型。
