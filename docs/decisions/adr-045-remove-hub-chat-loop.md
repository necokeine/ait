# ADR-045：移除 Hub、Chat 与 Loop 接口

- 状态：Accepted
- 日期：2026-09-25
- 决策来源：用户明确要求这三组不再支持，直接删除。
- 修订：ADR-026 的协议支持范围、ADR-044 的前端 Rust transport 方法映射。

从 Rust `MethodGroup` 和 `PASEO_METHODS` 删除 Hub 7 项、Chat 7 项、Loop 5 项，同时从前端 Rust transport 映射删除全部对应方法。不提供别名、占位处理器或迁移响应；HTTP server info 不发布这些 capability，WS optional capability 不协商，required capability 拒绝，已握手请求返回 `method_not_found`。

固定 Paseo 的原始 205 项 fixture 保持原样用于上游审计；仅在测试中维护 19 项显式排除清单。校验要求 registered 与 excluded 不相交，且 registered 等于 upstream 减 excluded。运行期不加载排除清单。

范围缩减后为 186 个上游名称，合并三个已有别名后为 183 个规范方法。已有 157 个业务处理器不变，剩 26 个占位（Plugin 15、Schedule 9、Browser 2）；加七个自定义方法，生产发布 190 项、实现 164 项。

该决策不更改 Message、Session 或 Run 边界，也不删除通用 EventHub 或 Skills 的 legacy 安装目录清理逻辑。导入的上游源码与 SDK 历史类型不代表 Rust 服务端支持。验证见 [实施报告](../reports/server-removed-groups.md)。
