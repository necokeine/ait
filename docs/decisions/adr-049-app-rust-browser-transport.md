# ADR-049：独立 App 的 Rust server 浏览器连接

- 状态：Accepted
- 日期：2026-09-26
- 范围：`apps/app`、独立 `server` 的 HTTP/WebSocket 传输边界与本地开发入口。

## 决策

沿用 ADR-044 的四连接 Rust 协议适配器。Electron 经主进程注入 Bearer，原生客户端经
WebSocket header 认证；普通浏览器先向 `POST /v1/auth/ws-ticket` 发送 Bearer，取得
30 秒有效、绑定页面 Origin、单次使用的随机票据，再以 `ait.ticket.<ticket>` subprotocol
升级 `/v1/ws`。服务端回显该 subprotocol；长期 Bearer 不进入 URL 或 WebSocket subprotocol。
每个物理连接独立换票，重连重新换票；关闭连接取消尚未完成的 HTTP 请求。

换票要求正确 Host、获准 Origin 和 Bearer，响应 `Cache-Control: no-store`；仅换票路由
提供 CORS preflight 和精确 Origin 响应，不开放通配来源或 cookies。票据仅在当前 API
实例内存中保存，最多 256 个，换票时清理过期项，校验时移除；过期、重放、不同 Origin
均失败。HTTP 查询参数仍禁止携带凭据，其他接口的 Bearer 要求不变。

服务端默认仍只允许自身 HTTP 来源，新增重复的 `--web-origin` 或 TOML `web_origins`
显式允许其他本地 HTTP 页面端口。配置只接受规范化 localhost、127.0.0.1、[::1] 来源，
保持 loopback listener 与 Host 防护。配置在创建监听器及数据目录之前验证。

`npm run dev:app`（也可 `npm run web --workspace=@getpaseo/app`）构建共享依赖和 Rust binary，
启动本地 server 与 Expo Web，默认分别为 7316、8081。server 数据单独存放 `.tmp/app/server`；
入口要求 `AIT_SERVER_TOKEN`，仅给 server 子进程传递，不注入 Expo public 配置。前端通过
原有直接连接表单输入令牌，沿用已有 Host 配置持久化。启动器仅管理自己创建的子进程，
任一退出或收到退出信号时关闭其余进程，超时强制回收。

## 边界

本次不改变领域模型、Provider 执行或 RPC 能力。ADR-001 v4 的 Message/Session/Run 约束
继续成立。Origin/票据属于 server-api transport，不下沉至 domain 或应用服务。

远端监听、HTTPS 部署、移动设备公网访问、SSH/relay/IPC 的 Rust 化不在此决策范围。
原生 App 继续使用直接 TCP Bearer 通道；Android 可用 `adb reverse` 访问本机 server。
本决策补齐 ADR-044、ADR-048 当时明确未实现的普通浏览器认证。

验证与限制见[实施报告](../reports/app-rust-server.md)。
