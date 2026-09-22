# Paseo WebSocket 接口移植：第十阶段

- 日期：2026-09-23；分支：`new`。
- 基线：`e4f57ce`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Forge、PR 与检查状态分组的 10 个规范 WebSocket 方法。累计已接通 81 个规范方法，剩余 110 个
catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `forge.search.request` | 同名 | neutral issue/change-request 搜索、kind aliases、limit 与 auth state |
| `github.search.request` | `github_search_request` | GitHub compatibility kind/availability payload |
| `checkout.pr.create.request` | `checkout_pr_create_request` | 当前 branch push 后创建 PR，返回 URL/number |
| `checkout.pr.merge.request` | `checkout_pr_merge_request` | 解析当前 PR，支持 merge/squash/rebase |
| `checkout.pr.status.request` | `checkout_pr_status_request` | 当前 PR、mergeability、review decision 与 check rollup |
| `checkout.pr.timeline.request` | `pull_request_timeline_request` | review、general comment、inline thread、稳定排序与 truncation |
| `checkout.forge.set_auto_merge.request` | 同名 | neutral auto-merge compatibility surface |
| `checkout.github.set_auto_merge.request` | 同名 | GitHub compatibility surface，共用实现 |
| `checkout.forge.get_check_details.request` | 同名 | check output、annotation 与 failed job |
| `checkout.github.get_check_details.request` | 同名 | GitHub compatibility surface，共用实现 |

旧下划线名称不作为 wire alias。请求和响应继续使用统一 envelope，Paseo payload 位于 `params`/`result`，不复制
第二层 `requestId`。两个 GitHub compatibility 名称仍分别发布，因为 Paseo 客户端把它们作为独立 capability；
neutral 与 compatibility 入口共享业务实现，不复制 adapter。

## 实现边界

`server-protocol::forge` 复制 Paseo search item、auth state、PR status/check、timeline review/comment/location、
annotation/failed job/check details 及 mutation DTO。`server-ports::forge::ForgeRuntime` 是全新的阻塞边界；
`server-application::forge::Forge` 负责显式 metadata 和 auto-merge request shape 的准入；API 只负责 canonical
method 路由、Paseo payload 投影及 inline error。生产 binary 组装 `server-workspace::LocalForge`，没有使用旧
Ait domain/application/adapter。

本地 adapter 使用 `gh` 和 Git 参数数组，不经过 shell；stdin 关闭，Git prompt 和 GH prompt 关闭。读操作
deadline 为 30 秒，写操作为 120 秒，stdout 上限 4 MiB，stderr 上限 64 KiB。`gh` 继承用户已有凭据；非
github.com host 设置 `GH_HOST`，用于已经由 `gh` 配置的 GitHub Enterprise。

search 分别调用 issue/PR list，保留一侧成功结果，按 ISO `updatedAt` 降序合并并应用总 limit；Paseo legacy
kind 被归一为 issue/change-request，GitHub compatibility 响应再把 change request 投影为 `pr`。status 使用
current branch 的 `gh pr view`，解析 draft/merged/mergeable/review/check rollup，并计算 none/pending/success/
failure。timeline 使用 GraphQL 读取首批 100 个 review、comment 和 review thread，保留 inline position、thread
resolution 与分页 truncation，按 createdAt/id 排序。

PR create 从显式或 `origin/HEAD`/main/master 推导 base，真实执行 `git push -u origin <head>`，再调用 GitHub
pull API。merge 和 auto-merge 解析 current PR number 后使用 `GH_PROMPT_DISABLED=1` 的 `gh pr merge`。check
details 读取 check-run、最多 20 条 annotation 和 workflow 的最多 100 个 job，只返回前 5 个失败 job。

## 与 Paseo 的对齐和差异

1. canonical method、camelCase、null/omission、legacy search kind、auth state、inline checkout/timeline error、
   auto-merge 参数规则、search 排序、timeline identity 校验、check identity 校验和 create-before-forge push 均以
   固定 Paseo 实现及其测试为基准。
2. Paseo forge registry 支持 GitHub、GitLab、Gitea、Forgejo 和 Codeberg，并探测 self-hosted forge。当前只有
   GitHub/已配置 GHES adapter；其他 forge host 尚无对应 CLI/parser，不能声称 neutral 方法已达到多 Forge 等价。
3. Paseo 在 PR title 或 body 缺失时调用 Provider-backed `gitMetadataGenerator`。独立 server 尚无 Provider
   runtime，所以要求两者显式非空，并返回 `UNKNOWN / Pull request title and body are required`，不会生成低质量
   占位文本。
4. Paseo current-PR resolver结合 head SHA、fork owner、parent repository、all-state candidates 与 cache，避免
   stale branch 名匹配。当前使用 cwd 下的 `gh pr view`，对同名 fork branch、renamed repository 和 stale PR 的
   选择不及原版完整。
5. Paseo 额外读取 GitHub merge-policy、merge queue、auto-merge permission 和 viewer capability GraphQL facts，
   在 direct merge/auto-merge 前做精确 preflight，并投影 `forgeSpecific`/legacy `github`。当前这些字段省略，命令
   交由 `gh`/GitHub 最终校验；错误仍返回 inline `UNKNOWN`，但提示时点和文案可能不同。
6. Paseo Forge service 有 TTL cache、in-flight 去重、按 GraphQL cost 预算的 batch PR polling、stale-success
   fallback 和 mutation invalidation。当前每次请求直接执行 bounded CLI，没有后台 poll、status update 或 cache。
7. Timeline 的 review/comment/thread shape、排序和 page truncation 已对齐；Paseo 还会把 GitHub attachment 原始 URL
   替换为可读取的 rendered URL。当前保留原 Markdown body，私有 attachment 的可显示性可能不同。
8. Check details 已对齐 check-run output、annotation、workflow job 筛选与 truncation；Paseo 会下载并截取最多五个
   失败 job 的日志。当前不下载日志，所以 `logTail`/`logTruncated` 省略；也没有 GitLab pipeline stage/job tree。
9. search 没有 Paseo 的 Enterprise query host normalization 和 read cache；unknown 的一侧 CLI failure 与原版一样
   不阻断另一侧结果。非 github.com 的 parseable remote 当前按已配置 GHES 交给 `gh`，不能像完整 registry 一样
   准确识别任意 self-hosted GitLab/Gitea。
10. 所有 CLI 网络行为使用本机 Git/`gh` config、credential helper 与环境 token。测试使用本地 bare remote 和
    受控 `gh` executable，未访问真实 GitHub/GHES，也未覆盖代理、rate limit、SSO 或 credential expiry。
11. 当前 Forge use case 复用 API 的全局 blocking semaphore，并由 Forge mutex 串行化所有 repository；Paseo 的
    cache/in-flight/mutation coordination 粒度更细，不相关 repository 可以并行。

## 测试执行

对应移植的 Paseo 测试包括：canonical capability 与 request/result shape；legacy search kind normalization；
neutral/compatibility search projection；title/body 和 auto-merge shape 准入；status/check/timeline DTO 投影；remote
URL 解析；no-remote、CLI-missing 和 unauthenticated 区分；issue/PR merge/sort；check rollup；timeline thread/
truncation；annotation/failed-job；merge/enable/disable command；以及真实 binary 通过一个 WebSocket 完成全部 10 个
request，并用本地 bare remote 验证 PR create 的真实 push。

本阶段新增 14 个新 server 测试：protocol 3、application 3、API 3、受控 CLI adapter 4、真实 binary WebSocket
1。新 server 覆盖率运行通过 12 个 test target、268 个测试，无失败或忽略；生产 Rust 代码行覆盖率为
12,634/14,798（85.38%），其中本阶段 API/application/workspace 三个可执行 Forge 模块为
1,278/1,702（75.09%）。相比第九阶段 86.73% 的全量覆盖率下降 1.36 个百分点，主要新增未由受控 CLI fixture
穷举的错误、超时、分页和 GraphQL 兼容分支；这些边界没有从统计中排除。

完整 workspace 回归通过 760 个测试，0 失败、5 忽略；workspace default-feature clippy、变更 server package
all-feature clippy、`cargo fmt --all -- --check` 与 `git diff --check` 均通过。可复现命令、逐 crate 数字和基线记录
在 [phase 10 coverage artifact](paseo-websocket-surface-phase-10-coverage.json)。
