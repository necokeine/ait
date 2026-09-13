## ADR-001：Skill、Skill Market 与 Skill Group

- 状态：Proposed（本阶段仅完成需求与方案，不授权实现）
- 日期：2026-09-10
- 依赖：NEC-150 核心领域模型 v4、ADR-009 AgentProvider 配置层、NEC-224 按记录持久化
- 来源：NEC-240

## 1. 背景与目标

Ait 已有版本化 Agent、固定 Agent 配置的 Run，以及 provider-neutral 的 `skill` 工具定义，
但目前没有用于创建、安装、组织、分配和运行 Skill 的领域模型。工具定义本身不等于已经安装 Skill，
也不能回答以下问题：

- 本机有哪些 Skill，它们来自哪里，当前内容是什么；
- 哪些 Skill 对某个 Agent 可见，运行开始后内容能否变化；
- 如何从外部目录发现 Skill，而不把未审查内容直接交给 Agent；
- 如何把经常一起使用的 Skill 作为一组复用；
- Codex、Claude 或 API Provider 如何在不扩大权限的前提下获得一致的 Skill 能力。

本设计引入三个一级控制面概念：

1. **Skill**：可安装、可审查、可版本化的指令资源包。
2. **Skill Market**：发现可安装 Skill 的来源目录与查询边界。
3. **Skill Group**：可绑定到 Agent 的具名 Skill 集合。

目标是让成员能够完成“发现或导入 → 审查 → 安装 → 分组 → 分配给 Agent → 在 Run 中按需加载”
的完整闭环，同时保持 Ait 现有的 Run 快照、权限、Project 隔离和审计不变量。

## 2. 非目标

本阶段明确不包含：

- 实现数据库迁移、HTTP/CLI/IPC、桌面页面或 provider adapter；
- 把 Skill 当作插件、MCP Server、工具实现或可执行安装脚本；
- 让 Skill 声明或自动获得文件、网络、Shell、凭证等权限；
- 付费市场、评分、评论、作者结算、账号同步或云端发布；
- Skill 依赖解析、嵌套 Skill Group、Skill Group 工作流编排；
- Project 或 Session 级别的 Skill 增删覆盖；MVP 只从 Agent 绑定解析；
- 自动执行从市场搜索到的内容，或在没有确认的情况下静默更新已安装 Skill。

## 3. 术语与职责边界

### 3.1 Skill

Skill 是 provider-neutral 的指令资源包，由一个主文档和零个或多个支持文件组成。它描述“何时使用、
如何完成某类任务”，但不实现工具，也不授予权限。

```text
Skill {
  id, name, description,
  enabled,
  current_revision_id,
  source?,
  created_at, updated_at
}

SkillRevision {
  id, skill_id, revision,
  manifest,
  primary_content,
  content_hash,
  created_at
}

SkillFile {
  skill_revision_id,
  path, media_type,
  size, sha256,
  content_ref
}
```

- `name` 在 Ait 安装级目录内唯一，是 Agent 调用 `skill(name)` 时使用的稳定名称。
- `description` 是触发与选择提示；运行开始时可进入可用 Skill 摘要目录。
- `primary_content` 对应规范化后的 `SKILL.md` 完整内容。
- `SkillRevision` 不可变；编辑、覆盖导入或刷新会创建新修订并原子推进
  `current_revision_id`，而不是原地改写历史内容。
- `source` 保存来源类型、源 URL/路径、外部标识、ref/version 等非秘密 provenance；本地手工创建可为空。
- `enabled=false` 使后续 Run 不再解析该 Skill，但不破坏历史 Run 快照。

Skill 不是 Agent。Agent 决定“由谁、使用什么模型和工具策略运行”，Skill 提供可复用的任务方法；
同一 Skill 可以分配给多个 Agent，同一 Agent 可以使用多个 Skill。

### 3.2 Skill Market

Skill Market 是“可发现 Skill 的来源”，不是已安装 Skill 的存储，也不参与 Run 执行。

```text
SkillMarket {
  id, name,
  kind,
  endpoint?,
  enabled,
  config,
  created_at, updated_at
}

MarketSkillEntry {               // 查询投影，不是本地领域事实
  market_id, external_id,
  name, description,
  author?, version?, updated_at?,
  source_ref,
  metadata
}
```

- Market adapter 负责搜索、读取元数据和获取一个候选资源包。
- 搜索结果是瞬时、可过期且不受信任的投影；只有显式安装后才创建本地 Skill 与首个 Revision。
- MVP 提供固定 adapter：GitHub URL、本地 `.skill`/`.zip`，以及能稳定提供搜索/下载契约的
  `skills.sh`、ClawHub。直接 URL/本地归档属于导入来源，即使该来源没有搜索能力。
- MVP 不接受任意自定义 HTTP 市场协议；新增市场 adapter 必须通过后续设计明确认证、限流、
  完整性与错误语义。`SkillMarket.endpoint` 仅供受支持 adapter 使用，不表示通用插件入口。
- 市场不可用不影响已安装 Skill 或正在运行的 Run。

安装成功后，运行只依赖本地 Revision；`source` 用于展示、检查更新和显式刷新，不能在 Run 开始时
临时联网拉取内容。

### 3.3 Skill Group

Skill Group 是一个具名、可复用的 Skill 集合，用于把一套能力分配给多个 Agent。

```text
SkillGroup {
  id, name, description,
  enabled,
  revision,
  created_at, updated_at
}

SkillGroupMember {
  group_id, skill_id,
  created_at
}
```

- Group 成员是集合，无执行顺序；运行时按 Skill `name` 稳定排序。
- MVP 禁止 Group 包含 Group，避免循环、跨层优先级和隐式大规模变更。
- Group 不是 workflow、squad 或 agent team；它不会自动按顺序调用 Skill。
- 修改 Group 增加 `revision`。未来 Run 看到新成员，历史 Run 保留已解析快照。
- 禁用 Group 后，未来 Run 忽略其全部成员；禁用其中一个 Skill 时仅排除该 Skill。

## 4. Agent 绑定与有效 Skill 集

Agent 同时支持直接 Skill 绑定和 Skill Group 绑定：

```text
AgentSkillBinding { agent_id, agent_revision, skill_id }
AgentSkillGroupBinding { agent_id, agent_revision, skill_group_id }
```

绑定变化属于 Agent 配置变化，必须增加 Agent revision。Skill 或 Group 自身更新不会伪造 Agent revision，
而由 Run 的解析快照记录真实有效内容。

创建 Run 时，application 在同一个一致性读取中：

1. 读取已固定的 Agent revision；
2. 展开该 revision 的直接 Skill 与 Group 绑定；
3. 忽略禁用的 Group 与 Skill；
4. 按 `skill_id` 去重，并按 Skill `name` 稳定排序；
5. 固定每个 Skill 的 `current_revision_id`、Group revision 和内容 hash；
6. 把结果写入 Run 的不可变 Skill 快照后才开始调用 Agent。

```text
RunSkillSnapshot {
  run_id,
  skill_id, skill_revision_id,
  source: direct | group,
  source_group_ids[],
  name, description,
  content_hash
}
```

同一 Skill 经直接绑定和多个 Group 重复出现时只暴露一次，同时保留全部来源用于 UI 解释。若两个不同
Skill 出现相同名称，绑定写入或 Run 解析必须失败；不得依赖排序选择其中一个。

Agent 页面必须展示“直接绑定”“来自 Group”和最终有效集合，避免 Group 修改造成不可见的能力漂移。

## 5. 运行时加载契约

### 5.1 渐进式披露

参考 Multica 的设计，Run 启动时只向 Agent 提供每个有效 Skill 的 `name + description` 摘要，
Skill 主内容在 Agent 明确调用 `skill(name)` 后加载，支持文件再按需读取。这样避免把所有 Skill 正文
一次性塞入上下文。

provider-neutral 语义固定为：

```text
loadSkill(run_id, exact_name) -> {
  skill_revision_id,
  primary_content,
  files: [{ path, media_type, size, sha256 }]
}

readSkillFile(run_id, skill_revision_id, path) -> bytes | text
```

- 只能加载该 Run 快照内的 Skill；同名的新增或更新版本对当前 Run 不可见。
- 精确名称不存在时返回稳定错误，不进行模糊选择。
- 每次加载及支持文件读取进入 Run 的操作时间线；不得把 Skill 内容伪装成人类 Message。
- API Provider 可使用现有 `skill` 工具契约；Codex/Claude 等原生 adapter 可以物化 Run 专属只读目录，
  但必须消费同一快照，不能写入或依赖用户全局的 skills 目录。
- 如果 Provider 不支持 Skill 工具或等价机制，Run 在调用前返回 capability 错误，不静默忽略已绑定 Skill。

### 5.2 指令与权限优先级

Skill 内容是受信任级别低于 Ait 系统策略、管理员策略、Project 指令快照和当前明确用户请求的辅助指令。
它不能：

- 覆盖审批、sandbox、工作目录或工具 allowlist；
- 读取未通过宿主授权的文件、环境变量或凭证；
- 改写 Message 历史、Agent 配置或其他 Skill；
- 因正文声称“必须执行”而绕过用户确认或宿主权限。

Skill 声明需要某个工具或能力时，该声明只用于兼容性检查和 UI 提示。实际权限仍由 Agent tool policy、
Run policy 与宿主审批的交集决定；缺少能力时应明确失败或提示，而不是自动提权。

## 6. 内容格式与校验

MVP 兼容常见的 `SKILL.md` 包结构：主文件包含 YAML frontmatter 与 Markdown 正文，支持文件使用相对路径。

最低 frontmatter：

```yaml
---
name: code-review
description: Review code changes when the user requests a review.
---
```

校验规则：

- `name` 规范化后非空，仅允许可移植字符集，并在安装级目录中唯一；
- `description` 必填并设置长度上限；
- 包内必须且只能解析出一个主 `SKILL.md`，支持外层单目录包装；
- 支持文件路径必须是清理后的相对路径，拒绝绝对路径、`..` 穿越、符号链接和大小写碰撞；
- 支持文件不能再次命名为 `SKILL.md`；
- 限制压缩包大小、解压后总大小、单文件大小、文件数和压缩比；
- 默认只把 UTF-8 文本与显式允许的媒体类型暴露给 Agent；未知二进制文件拒绝安装；
- 计算每个文件和整个 Revision 的 SHA-256，安装提交前完成全部校验；
- 解析 frontmatter 时忽略未知字段并保留原文；Ait 自有能力字段使用版本化命名空间，避免把第三方扩展
  误解释为权限。

建议沿用 Multica 当前安全基线：上传压缩包上限 16 MiB，解压后 bundle 上限 8 MiB，单文件 1 MiB，
最多 256 个文件；若实现阶段需要调整，必须在 API、CLI 与 UI 中使用同一组常量。

## 7. 生命周期与用户流程

### 7.1 手工创建

成员填写名称、描述与 `SKILL.md` 内容，可再添加支持文件。保存前预览规范化结果与校验问题；首次保存
创建 Skill 和 Revision 1。

### 7.2 搜索与安装

1. 用户在启用的 Market 中搜索；结果明确显示来源、作者、版本和更新时间（来源未提供的字段显示未知）。
2. 打开候选项后先查看主内容、支持文件清单、大小、hash 与来源。
3. 用户显式确认安装；系统下载到临时隔离区，校验完整包，再以一个事务创建本地 Skill/Revision。
4. 安装不自动绑定任何 Agent，也不自动执行内容。

市场只返回 URL 而不能提供预览内容时，UI 必须把“预览将在下载后进行”显示为独立步骤；下载仍不等于安装。

### 7.3 名称冲突

默认 `fail`，不改变本地状态。用户可显式选择：

- `overwrite`：保留本地 Skill ID 和绑定，创建新 Revision 并更新 provenance；
- `rename`：以可预测后缀创建新的 Skill；
- `skip`：保持现状并返回无变化结果。

任何冲突结果都返回现有 Skill 的 ID 与名称。不得通过自动改名悄悄规避冲突。

### 7.4 更新与刷新

- 手工编辑创建新 Revision，并在差异预览确认后推进 current revision。
- 有可刷新 provenance 的 Skill 可执行“检查更新”和“刷新”；检查更新只读，不修改内容。
- 刷新必须显式确认，保留 Skill ID、历史 Revision、Agent/Group 绑定，原子替换当前 Revision 与支持文件集。
- 上游改名若与本地名称冲突则整体失败；下载或校验失败也保持旧 Revision 可用。
- 支持回退到任一历史 Revision；回退本身创建一个内容相同的新 Revision，保持 revision 单调递增。

### 7.5 禁用与删除

- 禁用是首选的可恢复操作，只影响之后创建的 Run。
- Skill 被 Agent 或 Group 引用时，删除返回 `SKILL_IN_USE` 并列出引用者；用户先解除绑定再删除。
- Group 被 Agent 引用时同样返回 `SKILL_GROUP_IN_USE`。
- 删除不移除历史 Revision、Run 快照或审计记录所需内容；物理内容由保留策略在无引用后回收。

### 7.6 Group 管理与 Agent 分配

成员可创建、重命名、禁用 Group，批量增删成员，并在 Agent 编辑页增加或移除直接 Skill/Group。
`add` 是增量操作，`set` 是 replace-all；UI 和 CLI 必须明确区分，replace-all 在提交前展示将被移除的绑定。

## 8. 控制面、存储与并发

遵循 NEC-224，Skill、Revision、File、Market、Group 和绑定均为具名记录/关系表，不加入新的 Workspace
Snapshot blob。建议逻辑所有权如下：

- 全局目录：Skill、SkillRevision、SkillFile 元数据、SkillMarket、SkillGroup、Agent 绑定；
- 内容存储：较小 UTF-8 正文可随 Revision 记录保存，较大或二进制内容进入既有内容寻址附件存储；
- Project 数据库：只保存引用到这些全局资源的 RunSkillSnapshot，不复制可变目录记录。

所有写操作使用 expected revision/CAS：

- Skill 更新同时写入新 Revision、推进 current revision 和 outbox event；
- Group 成员 replace-all 与 Group revision 增加原子提交；
- Agent Skill/Group 绑定与 Agent revision 增加原子提交；
- Run 创建与有效 Skill 集解析必须看到一致快照，不能混合更新前后的 Group 或 Skill revision。

导出 Project 时必须包含历史 Run 所需的 Skill Revision 内容与 provenance 快照，但不包含市场凭证或本机路径。
导入后这些 Revision 只保证历史可读；若要供新 Run 使用，用户需选择复用已安装 Skill 或把归档内容安装
为新的本地 Skill。

## 9. 最小操作契约

命名保持 entity/operation 风格；具体 HTTP 路由与 DTO 在实现 issue 中固化。

```text
listSkills(filters)
getSkill(skill_id, revision_id?, include_content?)
createSkill(name, description, primary_content, files, expected_catalog_revision)
updateSkill(skill_id, patch, expected_revision)
checkSkillUpdate(skill_id)
refreshSkill(skill_id, expected_revision)
disableSkill(skill_id, expected_revision)
deleteSkill(skill_id, expected_revision)

listSkillMarkets()
searchSkillMarkets(query, market_ids?, cursor?)
inspectMarketSkill(market_id, external_id)
installMarketSkill(source_ref, conflict_policy, expected_catalog_revision)
importSkill(url_or_archive, conflict_policy, expected_catalog_revision)

listSkillGroups()
getSkillGroup(group_id)
createSkillGroup(name, description, skill_ids)
updateSkillGroup(group_id, patch, expected_revision)
setSkillGroupMembers(group_id, skill_ids, expected_revision)
deleteSkillGroup(group_id, expected_revision)

addAgentSkills(agent_id, skill_ids, expected_agent_revision)
setAgentSkills(agent_id, skill_ids, expected_agent_revision)
addAgentSkillGroups(agent_id, group_ids, expected_agent_revision)
setAgentSkillGroups(agent_id, group_ids, expected_agent_revision)
getAgentEffectiveSkills(agent_id)
```

所有 list/search 支持分页；读取 Skill 默认只返回元数据与文件清单，只有明确请求才返回正文，避免大型 Skill
让普通列表失控。

## 10. 稳定错误语义

至少定义以下错误码：

- Skill：`SKILL_NOT_FOUND`、`SKILL_NAME_CONFLICT`、`SKILL_DISABLED`、
  `SKILL_IN_USE`、`SKILL_REVISION_NOT_FOUND`、`SKILL_INVALID_MANIFEST`、
  `SKILL_CONTENT_TOO_LARGE`、`SKILL_PATH_INVALID`、`SKILL_HASH_MISMATCH`；
- Market/import：`SKILL_MARKET_NOT_FOUND`、`SKILL_MARKET_DISABLED`、
  `SKILL_MARKET_UNAVAILABLE`、`SKILL_SOURCE_UNSUPPORTED`、`SKILL_SOURCE_CHANGED`、
  `SKILL_REFRESH_UNAVAILABLE`；
- Group/binding：`SKILL_GROUP_NOT_FOUND`、`SKILL_GROUP_NAME_CONFLICT`、
  `SKILL_GROUP_IN_USE`、`SKILL_BINDING_CONFLICT`；
- Run：`RUN_SKILL_NOT_AVAILABLE`、`RUN_SKILL_CAPABILITY_UNSUPPORTED`、
  `RUN_SKILL_SNAPSHOT_CONFLICT`。

网络超时、市场限流和暂时 I/O 可重试；manifest、路径、名称冲突、权限与引用不变量错误不可重试。

## 11. 桌面信息架构

Settings 增加 **Skills** 一级页面，包含三个页签：

- **Installed**：搜索/筛选本地 Skill，查看内容、文件、来源、hash、Revision、引用 Agent/Group，执行
  创建、导入、刷新、禁用和删除；
- **Markets**：选择市场、搜索候选、查看详情与安全提示后安装；
- **Groups**：维护 Group 及成员，查看引用 Agent。

Agents 页面增加 Skill 配置区，分别编辑直接 Skill 和 Group，并实时展示“有效 Skill”预览及来源。
Run 详情页显示固定的 Skill Revision 清单和本次实际加载记录，使用户能回答“这个结果用了哪个版本”。

必须覆盖 loading、空列表、无结果、市场离线、部分市场失败、冲突、校验失败、并发 revision conflict、
Skill/Group 被引用等状态。多个市场搜索时，一个市场失败不能抹掉其他市场的结果，但页面必须明确标出
不完整结果。

## 12. 功能验收标准

### Skill

- 可创建含主文档与支持文件的 Skill，读取默认仅返回元数据，按需读取正文。
- 更新产生不可变新 Revision；历史 Run 仍能读取其固定版本。
- 非法路径、重复主文件、超限资源包和名称冲突不会留下半安装记录。
- 禁用 Skill 只影响新 Run；被引用 Skill 不能直接删除。

### Skill Market

- 可搜索至少一个有搜索能力的受支持 Market，并可从 GitHub URL 和本地归档导入。
- 候选 Skill 在确认安装前可查看来源与内容/文件清单；搜索结果不会自动执行或绑定。
- 安装、刷新与四种名称冲突策略返回结构化结果；失败保持原状态。
- 市场离线不影响已安装 Skill 和历史 Run。

### Skill Group 与 Agent

- 可创建非嵌套 Group，批量管理成员，并绑定到多个 Agent。
- 直接绑定和 Group 展开的重复 Skill 在有效集合中只出现一次，来源可解释。
- 绑定变化增加 Agent revision；Group/Skill 内容变化在新 Run 中生效，但不改变活动或历史 Run。
- replace-all 操作不会伪装成 add，客户端能预览被移除项。

### Run

- Run 创建时原子固定 Skill、Group 和 Skill Revision 快照。
- Agent 只看到该快照的摘要，只能按精确名称加载其中的 Skill。
- Skill 加载遵循既有权限与工具策略，不能增加 Run 权限。
- Run 详情能显示可用 Skill、固定 Revision 与实际加载记录；更新或删除目录记录不破坏历史审计。
- 不支持等价 Skill 机制的 Provider 在调用前明确报 capability 错误。

## 13. 质量与安全验收

- 单元测试覆盖 manifest、路径清理、压缩炸弹限制、hash、去重、稳定排序、Group revision 与错误映射。
- 存储测试覆盖不可变 Revision、引用删除保护、CAS 冲突、事件 outbox 与 Run 快照原子性。
- application/API/CLI/IPC/desktop contract test 对同一操作和错误使用一致语义。
- adapter 测试证明 Codex/Claude 物化目录是 Run 专属、只读且不污染用户全局目录；API Provider 测试
  证明 `skill` 工具只读取快照内 Revision。
- 安全测试把 Skill 正文、frontmatter、文件名和市场元数据当作不受信任输入，覆盖路径穿越、符号链接、
  Unicode/大小写碰撞、HTML/Markdown 注入和敏感字段泄露。
- 离线与恢复测试证明市场故障、下载中断、daemon 重启不会留下 current revision 指向不完整内容的状态。

## 14. 建议实施切片

本 ADR 接受后再创建实现 issue，按可独立验收的纵向切片推进：

1. **本地 Skill 闭环**：领域模型、Revision/File 存储、create/get/list/update、校验与 CLI。
2. **Agent 绑定与 Run 快照**：直接 Skill 绑定、渐进式加载、审计和一个 API Provider 闭环。
3. **Skill Group**：Group CRUD、Agent Group 绑定、有效集合预览与桌面配置。
4. **导入与 Market**：本地归档/GitHub 导入、搜索 adapter、预览、冲突策略、刷新与离线行为。
5. **原生 Agent 适配与桌面完善**：Codex/Claude Run 专属物化、Installed/Markets/Groups 页面和恢复测试。

每个切片必须保持 Rust workspace 可构建，并沿 `domain <- ports <- application <- adapters` 的依赖方向；
不得为了某个 Provider 把 SDK、文件系统、HTTP 或 UI 类型放入 `domain`。

## 15. 待产品确认

以下问题不阻碍本设计评审，但会影响后续实施范围，接受 ADR 前应确认：

1. 首发必须支持哪一个可搜索市场：`skills.sh`、ClawHub，还是两者都支持？
2. 自建 Skill Market 是否进入近期路线；若需要，优先采用 Ait 定义的只读协议还是兼容某个既有协议？
3. Project 导入时，历史 Skill Revision 是只读归档，还是允许一键安装为新的本地 Skill？
4. 是否需要发布前的签名/作者信任状态；MVP 当前只承诺来源、hash、显式预览与权限不升级。
5. Skill 更新采用始终人工确认，还是允许用户对单个 Skill 显式开启自动更新？本设计默认始终人工确认。

在这些问题确认前，不应开始数据库或 API 实现；可先把它们拆成产品决策 issue。
