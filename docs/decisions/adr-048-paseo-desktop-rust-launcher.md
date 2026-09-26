# ADR-048：Paseo workspace 与 Rust 桌面服务启动

- 状态：Accepted
- 日期：2026-09-25

## 决策

根 npm workspace 管理 `apps/app`、`apps/paseo` 及固定 Paseo 源码中的 protocol、client、
relay、highlight、plugin、expo-two-way-audio 共享包。沿用上游 lockfile 后按实际 workspace
重新解析，并保留所需前端 patches。共享 Plugin 包仅满足已有前端编译依赖，不恢复 Rust
服务端 Plugin 接口；Node server、Node CLI 不进入 workspace。

Electron 主进程直接启动独立 Rust `server` binary，指定专属 data directory 和 loopback
监听地址。开发入口先构建共享包、主进程及 Rust binary；打包资源包含本机平台的 Rust
binary，跨平台打包须显式提供相同目标的 binary。

主进程每次启动生成独立随机 Bearer token，仅通过子进程环境传入，在内存中按当前托管
endpoint 给 WebSocket transport 注入。token 不返回 renderer、不进入主机持久配置或
URL。服务日志声明监听地址后，必须成功完成带认证的 Rust hello 握手才返回 running。
前端每次启动更新托管服务的连接地址，兼容系统分配的临时端口。

服务生命周期操作串行化，只管理实际创建的 ChildProcess，绝不根据 PID 文件接管或停止
其他进程。停止先 SIGTERM，超时 SIGKILL；启动失败回收子进程。Rust 数据目录自身的锁
负责阻止重复打开。服务随桌面正常退出而停止，移除旧的后台驻留开关；旧 Node CLI 安装
明确拒绝，诊断状态直接读取当前 Rust 子进程状态。

## 边界与限制

不改变 Rust domain 或 Agent 执行实现。普通浏览器认证、远端接入、Node CLI 功能、托管
服务独立于桌面长期后台运行均不属于这次启动整合。Electron 被 SIGKILL 或系统崩溃时
不能运行退出钩子；本轮不实现操作系统 supervisor 或孤儿进程接管。

验证结果见 [实施报告](../reports/paseo-desktop-startup.md)。
