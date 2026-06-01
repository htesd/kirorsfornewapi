# Changelog

## [v39] - 2026-06-01

### Fixes —— 孤儿 tool_result 导致超长对话上游 400

- **根因**：客户端（Claude Code）反复 auto-compact 超长对话时，会压掉发起 `tool_use` 的
  assistant 消息，却保留其 `tool_result`，在 history 中段残留**孤儿 tool_result**（有结果、无调用）。
  上游 Kiro 对 result-without-use 返回 `400 Improperly formed request`，导致长对话流式请求 100% 失败。
- **修复**：`src/anthropic/converter.rs` 新增 `remove_orphaned_tool_results`，与既有
  `remove_orphaned_tool_uses` 对称。在 convert 流程 step 9.5（移除孤儿 tool_use 之后）收集
  history 全部 `tool_use_id`，删掉 user 消息里无对应 tool_use 的 tool_result。
  - 此前 `validate_tool_pairing` 只清理"孤儿 tool_use"和"当前消息的孤儿 tool_result"，
    从不反向校验 **history 中段**的孤儿 tool_result —— 平时对话短不触发，388 消息超长对话才暴露。
- **验证**：用线上真实失败请求体（388 消息、193 对工具调用、1 个孤儿 `tooluse_kSZAyw…`）
  模拟，精确删除该孤儿（194→193），其余配对全部保留。新增单测
  `test_remove_orphaned_tool_results_midhistory` 覆盖中段孤儿 + 配对共存场景。
- **取证增强**：移除孤儿时 warn 日志带上被删的 tool_use_id 列表，便于线上复发定位。

### Notes & Caveats

- 纯后端修复，与 v38 的前端改动正交。对抗审查（Skeptic）通过：无 high 项；
  实测确认无重复 tool_use_id/tool_result_id；空数组经 `skip_serializing_if` 已不序列化。
- 已知局限（不阻塞）：按 ID 存在性匹配，不校验严格的 turn 内前置顺序；当前 Kiro 校验为存在性，足够。

## [v38] - 2026-06-01

### Features —— Admin UI 设置页重构 + 分组筛选 + 调度模式可视化

- **独立「设置」页取代 512px 小弹窗**：原先 `SettingsDialog` 把 调度策略 / 账号池分组 / API Key
  三块全挤进一个 `sm:max-w-lg` 弹窗，纵向堆叠、空间局促。改为顶部第三个 Tab「设置」
  （`settings-page.tsx`），全宽布局、Card 分区：宽屏下调度策略与账号池分组并排，API Key 单独成行。
  - 从弹窗抽出 API Key 管理为独立组件 `api-keys-panel.tsx`（去掉 Dialog 外壳）。
  - 删除 `settings-dialog.tsx`。
- **凭据列表按分组筛选**：列表头新增分组下拉（全部分组 / 各分组（含成员数）/ 未分组）。
  用 `groups[].credentialIds` 反查每个凭据所属组，筛选后再分页；统计卡片随筛选联动
  （「当前筛选 N / 共 M」）；切换筛选自动回第一页。此前所有账号一律平铺，账号多时难定位。
- **Header 调度模式改为只读徽章**：移除顶部「负载模式切换」「限流冷却」两个按钮。
  - 旧「负载模式切换」只在 `priority`/`balanced` 间切，**完全无视 `affinity`**：若当前是
    会话亲和（推荐模式），按钮误显示「均衡负载」，且点一下会把系统**踢出 affinity** —— 真正的 footgun。
  - 现改为只读展示当前模式（会话亲和 / 负载均衡 / 优先级固定），点击跳转设置页修改。
    所有调度参数统一收敛到设置页的 `SchedulingPanel`（已支持完整 3 模式 + 亲和参数 + 缓存放大）。

### Notes & Caveats

- 文案修正：账号池分组从「严格隔离」改为「分组路由（fail-open）」，与实际 fail-open 行为
  （组内账号全挂时回退全部账号）一致。
- 纯前端改动，后端 API 未动；`/config/load-balancing`、`/config/rate-limit-cooldown`
  端点及其 hook 保留（设置页经 `/config/scheduling` 统一读写，旧端点仍可用）。
- 已用 Playwright + mock 数据本地验证：设置页三卡片、分组筛选（FREE 池 → 2/共7）、
  只读模式徽章、标题不再竖排，均符合预期。

## [v36] - 2026-06-01

### Features —— 原生 thinking / reasoning 接入（reasoningContentEvent）

- **接入 Kiro 上游原生推理流**：新增 `reasoningContentEvent` 事件解析（`src/kiro/model/events/reasoning_content.rs`），
  转成 Anthropic 标准 thinking 块返回客户端。此前反代完全没解析这个事件类型，
  上游下发的推理内容（payload `{"text":...}`）被当 `EventType::Unknown` **整段丢弃**
  ——实测一次请求丢弃 453 个 reasoning 片段。
  - `base.rs`：`EventType::ReasoningContent` + `Event::ReasoningContent` 分发。
  - 流式（`stream.rs`）：`process_reasoning_content` 发 `content_block_start(type=thinking)`
    → `thinking_delta` → `signature_delta` → `content_block_stop`。`native_reasoning_seen`
    标志让正文绕过 fake `<thinking>` 标签解析。reasoning→text / reasoning→tool_use / 流末尾
    三处都先 `close_reasoning_block_if_open`。
  - 非流式（`handlers.rs`）：累积 reasoning 作为独立 thinking 块放最前；保留 fake 文本解析作 fallback。
- **Opus 全系默认开启 adaptive 思维链**：`override_thinking_from_model_name` 改为"所有 opus
  默认 adaptive + effort"（无需 `-thinking` 后缀），effort 客户端传入优先、缺省 high。
  旧实现硬编码 `is_opus_4_6`，导致 4.7/4.8 退化成 enabled、effort 丢失。

### Verified（实测）
- v36 上线后 opus-4.8 流式请求正确产出 thinking 块（thinking + thinking_delta + signature_delta + text）；
  reasoningContentEvent 未识别日志从 453 降到 0。
- thinking 为**真实推理**（数学题算出正确中间值；thinking 用英文、正文用中文，证明是内部推理流非安慰剂）。
- **effort 强度档位实测**（3模型×5档×3次）：effort 有效但天花板是 `high`。
  - opus-4.8：low 162 → medium 342 → high 521（thinking 字符均值，单调强递增）；xhigh/max ≤ high（上游静默钳位）。
  - opus-4.7：low 166 → high 386，同样 high 封顶。
  - opus-4.6：effort 基本失效（high/xhigh 档 0 thinking）。
  - 默认 high 正好卡峰值，xhigh/max 无增益。
- **上游真实模型 ID**（实测）：opus 4.5/4.6/4.7/4.8、sonnet 4.5/4.6、haiku 4.5 可用；
  sonnet 4.7/4.8 返回"模型不支持"。`-thinking` 后缀是反代/客户端侧概念，`map_model`
  按版本号映射时剥离，上游无任何 `-thinking` 变体——thinking 是模型开关，非模型品种。

### Notes
- 312 个测试全绿（含 reasoning 事件解析 + 流式 thinking 块转换的新测试）。
- 客户端侧：Claude Code 默认带 `redact-thinking-2026-02-12` beta header 隐藏 thinking（UI-only），
  需 settings.json 设 `showThinkingSummaries: true` 才显示。

## [v35] - 2026-05-31

### Features —— 账号池分组（严格隔离）+ 缓存倍率热调

- **账号池分组**：新增 SQLite `groups` + `credential_groups` 表 + `api_keys.group_id` 列
  （`src/db/groups.rs`，幂等迁移）。apikey 绑定分组后**严格隔离**——只能用该分组内的账号，
  组内全挂则请求失败（不回退全局池）。
  - 认证中间件（`middleware.rs`）解析 apikey→允许凭据 id 集合，经 `Extension(AllowedCredentials)`
    注入；provider `call_api_*_in_group` 透传到 `MultiTokenManager.acquire_context_with_session_and_group`
    → `select_by_session_affinity` / `select_next_credential` / busy-vs-disabled 统计全部加分组过滤。
  - 失败策略 **fail-open**：DB 解析失败时降级为不限制（放行），因分组定位是成本/缓存优化而非
    安全租户隔离，可用性优先（体量小，偶发错误路由可接受）。
  - Admin：`/groups` CRUD + `/credentials/{id}/group` + `/api-keys/{id}/group`；前端分组管理面板
    + 每个 apikey/账号的分组下拉。
- **缓存倍率全局热调**：`perceived_cache_hit_ratio` 从启动固定值改为 `MultiTokenManager` 的
  `Mutex<Option<f64>>`，经 `/config/scheduling` 端点 + 调度面板运行时热调（persist 落盘），
  每请求读 live 值。删除旧的 AppState→router 冗余传递链。

### Notes
- credential id 稳定（已有 id 保留回写、新增从 max+1），故用作分组映射 key 安全。
- 严格隔离的所有选号路径已逐一审计无组外泄漏（含 affinity 跨 apikey 共享 map、自愈重选、空分组 bail）。

## [v34] - 2026-05-31

### Features —— 会话亲和调度（Session Affinity Scheduler）
- **新调度模式 `affinity`，并设为默认**（`default_load_balancing_mode` 由 `priority` 改为 `affinity`）。
  动机：实测发现 **Kiro 前缀缓存按凭据/profileArn 隔离**——同样的上下文在体验过的账号上是热的
  （cache_read ≈ 0.754×），换到新账号则全冷（≈ 1.432×，约 1.9×）。`balanced` 模式把同一会话
  打散到不同账号，等于每次切号都触发冷启动，反复销毁缓存局部性。亲和调度让**同一会话尽量钉在
  同一账号**，把缓存命中维持在结构上限附近。
- **核心机制**（`select_by_session_affinity`，由 HRW 重写为有状态 v3）：
  - **新会话 → LRU**：在健康账号里选 `last_selected_at` 最旧的（"最久未调用首先调用"），让新会话
    均匀铺开，而不是全挤一个账号。`last_selected_at` 是纯内存字段，与持久化的 `last_used_at` 解耦。
  - **主账号健康 → 直接复用 primary**（保持会话黏性 / 缓存局部性）。
  - **主账号冷却或繁忙 → 踢到稳定次选 alt**，`alt_streak++`；连续命中达 `affinity_promote_threshold`
    （默认 3）次后**把 alt 转正为新 primary**——逐步排空热账号，避免所有重度会话回头争抢同一个号。
  - **TTL 淘汰**：`affinity_map` 按 `affinity_map_ttl_secs`（默认 1800s）对不活跃会话做 `retain` 清理，
    防止 map 无限增长。
- **会话标识**：`conversationId` 取自 `metadata.user_id` 的 session_id（Claude Code 单会话内稳定），
  缺失时回退随机 UUID。429 时 `report_rate_limited` 走 `disabled_until` + `DisabledReason::RateLimited`
  冷却，到期自愈。
- **超参数运行时可调 + Admin 面板**：新增 `affinity_promote_threshold`（K，clamp 1-20）、
  `affinity_map_ttl_secs`（clamp 60-86400）两个 `AtomicU32/U64` 字段，连同 `rate_limit_cooldown_secs`
  一起通过新的 `GET/PUT /config/scheduling` 端点热调整（`persist_config_field` 落盘）。前端新增
  `scheduling-panel.tsx`（mode 下拉 + K/TTL/cooldown 数字输入，失焦保存），嵌入设置弹窗。

### Verified
- `cargo check` 通过；293 个测试全绿（含 6 个 affinity 测试，新增 2 个：
  `session_affinity_new_sessions_rotate_via_lru`、`session_affinity_promotes_alt_after_threshold`）。
- v34 镜像（musl 交叉编译 + COPY 派生镜像）`--force-recreate` 一次重启成功，承载会话未断。
- 上线复查：`mode=affinity` 稳定，`rate_limited=0`（429 无反弹），同会话逐条稳定命中 ≈61%。

### Notes & Caveats
- **聚合命中率 ≈55%，不会冲到 80%**。复盘结论：~55-61% 是这批重度 agentic 流量的**结构性天花板**
  ——只有历史前缀可缓存，每轮新增的 `toolResults` / 新消息全价，约占 40%。亲和已把命中拉到能命中的
  极限，再往上是 Kiro 计费结构决定的，不是调度能解决的。80% 是早期基于轻工具合成实验定的乐观目标，
  对真实"重历史 + 大量 toolResults"流量不成立。
- 要进一步压成本需回到**另一条线**：动态工具子集（把约 81% 从不调用的"死重"工具移出 currentMessage），
  这才是对重度会话有增量的杠杆。
- 工具重户暂不特殊处理。`balanced` 模式保留但不推荐（缓存局部性问题）。

## [v30] - 2026-05-29

### ⚠️ 重大 Bug 修复（计费正确性）
- **NewAPI 双重计费 bug**：v20-v29 一直 emit `input_tokens=总值`，但 Anthropic API 规范
  要求 `input_tokens` = **仅未命中部分**，与 `cache_read_input_tokens` / `cache_creation_input_tokens`
  **互不重叠**。总上下文 = 三者之和。
- 结果：NewAPI 把缓存部分**计费两次**——总值按输入价、缓存值按缓存价。即便 v27 后我们
  inflate 到 95%，用户仍按"总输入 + 95% 缓存"被收。
- v30 在 `build_usage_json` 内做减法：`uncached_input = total - cache_read - cache_creation`，
  夹到非负。
- 修复后，95% inflation 场景下 NewAPI 看到的是：
  `input_tokens = 5% × 总` + `cache_read_input_tokens = 95% × 总`，账单按 5% 输入价
  + 95% 缓存价计算，**真正贴近用户感知的"95% 命中"折扣**。

### Verified（v30 上线后）
- 非流式 opus-4-7：input=294 / cache_read=5590 / 总=5884，**5.0% + 95.0%** ✓
- 流式 opus-4-7：input=294 / cache_read=5592 / 总=5886，**5.0% + 95.0%** ✓

### Notes
- DB 的 `prompt_tokens` 仍记总值（5884/5886），Admin UI 的"提示 N"列照旧；
  `cache_read_reported` 仍是 inflated cache_read 值。NewAPI 收到的 `input_tokens`
  = `prompt_tokens - cache_read_reported`，可在 UI 侧推算。

## [v29] - 2026-05-29
### Features
- **DB 新列 `cache_read_reported` + Admin UI 双值展示**：
  - SQL 迁移：`ALTER TABLE requests ADD COLUMN cache_read_reported INTEGER`
    （schema::init 用 pragma_table_info 守卫，幂等）。
  - `cached_tokens` 继续记**真实估算值**（保留给未来重拟合 baseline 用）。
  - `cache_read_reported` 新增记**实际发给 NewAPI 的值**（被 perceived_cache_hit_ratio
    放大后，与 stream `message_delta.usage.cache_read_input_tokens` 完全一致）。
  - 两条 emit 路径（非流式 / 流式）都已接：StreamContext 新增 `emitted_cache_read`
    字段在 `generate_final_events` 设置后由 SSE 流外层落库；非流式直接在 response
    构建后调用 `builder.set_cache_read_reported`。
  - Admin UI 列表：单格双行展示，绿色"真 N"=真实估算、蓝色"报 N"=实报；详情面板新增
    两个 Field 分别显示。
- **`/v1/models` 加入 claude-opus-4-8 + thinking 变体**（v28，Kiro 上游 5/29 已上）

### Verified
- 非流式 opus-4-7: cached=3631 / reported=5594 (0.950) ✓
- 流式 opus-4-7: cached=3500 / reported=5592 (0.950) ✓
- prod DB 已迁移加列，老行 cache_read_reported=NULL（UI 显示"-"），无中断

## [v27] - 2026-05-29
### Features
- **缓存放大策略改成"直接覆盖"**：`inflate_cache_read` 不再 `max(实际, 目标)`，
  只要命中就把 `cache_read_input_tokens` 直接设为 `round(prompt × ratio)`，
  夹到 `[0, prompt]`。配合 prod config `perceivedCacheHitRatio` 从 0.92 调到 0.95。
- **HIT_THRESHOLD 0.8 → 0.9 放宽**：边界 miss（ratio ∈ [0.8, 0.9]）现归类为命中、
  按 95% 缓存上报。代价：fresh 短请求可能被误判命中（代理方多承担成本），用户账单更便宜。
- **opus-4.8 baseline 复用 4.7**：Kiro 上游刚发布 opus-4.8（v26 上线），样本不足以独立
  拟合，暂用 4.7 系数 `(a=8.37e-6, b=296e-6, 0)` / thinking `(7.22e-6, 406e-6, 0)`。
  Anthropic 通常迭代版本单价差异 <10%，等积累 100+ 样本后用 sqlite + python OLS 重拟合。

### Verified（v27 上线后实测）
- `opus-4-8` 两次请求 `cache_read/input_tokens` = 0.9500 / 0.9501 ✓
- 285 个测试全绿（含 2 个新 opus48 baseline 测试）
- prod config 已写 `perceivedCacheHitRatio: 0.95`，备份 `.bak-v27`

## [v26] - 2026-05-29
### Features
- **claude-opus-4-8 支持上线**：Anthropic 发布 4.8 后第一次复测，Kiro 上游已可用 opus-4.8。
  - `claude-opus-4-8` / `claude-opus-4-8-thinking` 实测响应正常
  - `map_model` 加 `opus.*4-8|4.8` → `claude-opus-4.8` 分支
  - `get_context_window_size` 给 opus-4.8 配 1M 上下文（与 4.7 同）
- **sonnet-4-8 / haiku-4-8 暂不映射**：Kiro 上游返回 `INVALID_MODEL_ID`，还在 rollout。
  - sonnet-4-8 走 `None` → "模型不支持"，客户端可主动降级 4.6
  - haiku-4-8 沿用 haiku 历史兜底（任意 haiku → 4.5），保持兼容
  - 上游上线后只需在 `map_model` 加分支

### Notes
- v24/v25 是探测性部署（v24 误把全部 4-8 映射，全 400；v25 修正后发现 opus 通而 sonnet/haiku 不通），
  v26 是清晰可用的最终态。
- `cache_estimate.rs` 暂无 opus-4.8 baseline；Kiro 对 opus-4.8 应像 4.7 一样上报 `cacheReadInputTokens`
  真值（tokenUsageEvent 路径优先），baseline 仅在未来 fallback 路径需要时补，等样本积累后重拟合。

## [v23] - 2026-05-29
### Features
- **会话亲和调度（sticky routing by conversationId）**：
  - 痛点：balanced 模式下每个请求"least-used"选号，**同一对话被打到不同账号** →
    每个账号在 Kiro 服务端的 prefix cache 都是冷的，缓存优化基本作废。
  - 方案：对 `conversationState.conversationId` 用 **HRW(Rendezvous) 哈希** 在可用
    凭据里选号。同 conversationId 稳定锁同一账号；账号增减只影响原映射到下线
    账号的会话，其它不变。
  - 兼容：保留 balanced/priority 两个模式（粘性优先级最高，原选号逻辑作为降级路径）。
  - 实现：
    - `token_manager::select_by_session_affinity` 用 `hash(session_key || cred.id)`
      取 max 选号（HRW 标准做法）。
    - `acquire_context_with_session(model, session_key)` 包装：首次 attempt 用亲和；
      该凭据被并发占满/瞬时不可用时降级走原 balanced/priority。
    - `provider::extract_conversation_id_from_request` 从 Kiro 请求体 JSON 抽
      conversationId（converter 已用前 2 条 user 消息哈希得到，连续 turn 稳定）。
  - 3 个新测试覆盖：同 key 稳定选号 / 不同 key 分布 / 禁用后自动跳转。

### Notes & Caveats
- 同 conversationId 高并发时（同一对话短时间多 turn），并发占满会降级到其它账号，
  此请求丢失缓存命中——可接受（这种情况罕见，避免单账号被打死）。
- 客户端 `/compact` 会换 conversationId（前 2 条 user 变了），自然换账号——符合
  "上下文确实重建了"语义。

## [v22] - 2026-05-28
### Features
- **`clear_thinking_20251015` edit 支持**：v21 部署后实测 prod 流量含此 edit 类型
  （Anthropic 较新的 beta，清理推理 token 占的 context）。v22 实现等价裁剪：
  - 扫描 assistant 消息里的 `thinking` 块（无 id，按出现顺序）
  - 保留最后 `keep.value` 条（缺省 3），其余删除
  - 与 `clear_tool_uses_20250605` 互不冲突，同 `edits` 数组里可并列
  - 实现在 `src/anthropic/context_management.rs::clear_thinking`，3 个新单元测试

## [v21] - 2026-05-28
### Features
- **context_management beta（remote compact）实现客户端裁剪**：
  - v20 只接受字段 + warn，v21 起 proxy 侧"模拟"应用 `clear_tool_uses_20250605` 指令。
  - 行为：当 `payload.context_management.edits` 含此 edit 且当前估算 input_tokens
    达到 `trigger.value` 时，扫描所有 messages，把早期的 `tool_use` 块和配对的
    `tool_result` 块从历史里删除，保留最后 `keep.value` 条（默认 3）+ `exclude_tools`
    列表里的工具，从而显著降低发给 Kiro 的 token 数。
  - 空 message（清完只剩空数组）自动删除。
  - 实现在新模块 `src/anthropic/context_management.rs`（6 个单元测试覆盖触发阈值、
    保留逻辑、exclude_tools、空消息删除、未知 edit 类型兜底）。
  - 注：不上报 `applied_edits` 回执（多数客户端只关心副作用）；如客户端依赖回执
    可在 v22+ 扩展。
- **图像端到端实测**通过：opus-4-7 + base64 PNG，模型正确识别颜色（v20 已修代码，
  v21 实测确认通路 OK）；haiku-4-5 无视觉能力是 Anthropic 上游限制，不是 proxy bug。

### Verified
- 1×1 红色 PNG (base64) → opus-4-7 回复 "Muted reddish-pink." ✓
- 响应 `usage` 含 `cache_read_input_tokens: 5428` / `input_tokens: 5900` = **0.92**，
  perceived_cache_hit_ratio 放大功能验证 OK ✓

## [v20] - 2026-05-28

### 重大 Bug 修复
- **响应里漏发 `cache_read_input_tokens`**：v19 之前，无论检测/精确取到多少缓存命中，
  发给 NewAPI 的 `usage` 对象里**完全没有** `cache_read_input_tokens` / `cache_creation_input_tokens`
  字段——NewAPI 永远按全价计费，缓存优化白做。v20 在流式 `message_delta`、非流式
  response、cc 缓冲流三条路径上都补齐。

### Features
- **`perceived_cache_hit_ratio` 配置**（默认 None = 不放大）：
  反代实际命中受 Kiro 服务端缓存容量上限（~50-70%）制约，按真值上报 NewAPI 用户账单
  仍偏贵。运营方可在 `config.json` 配 `"perceivedCacheHitRatio": 0.92` 让 NewAPI 看到
  的命中比例稳定在 90-95%；代理方承担与 Kiro 真实计费的差额。
  - 仅在判定命中（`cache_read > 0`）时放大：`upload = max(实际, prompt × 比例)`，
    夹到 prompt 上限。未命中仍报 0。
  - 流式（`/v1`、`/cc/v1`）与非流式三路径统一应用。
- **`context_management`（Anthropic remote compact beta）字段接受 + 警告**：
  ` {edits:[{type:"clear_tool_uses_20250605", ...}]}` 之类指令现可正常反序列化，
  收到时记 warning（Kiro 后端不原生支持，已对照 kiro-gateway/AIClient-2-API 确认
  生态无人实现），客户端通常会本地回退应用同等裁剪。
- **图像支持加固**：
  - `ImageSource` 字段全部 optional：URL/file 源不再因缺 `data` 字段反序列化失败、
    把整个 image block 静默丢掉。新支持的 source 形态：`base64`（原样转）、`url` / `file`
    （记 warning、跳过，未来 v21+ 加 async pre-fetch 把 URL 转 base64 再喂 Kiro）。
  - **tool_result 里的图也抽出来**：浏览器/截图类工具回填的图片此前被丢，现按 kiro-gateway
    同款方式从 tool_result content 数组里再扫一遍 image 块。

### Design Rationale
- 三个反代项目（本项目 / kiro-gateway / AIClient-2-API）grep 确认 Kiro **无 remote compact**
  上游能力，本次只做"接受 + 不报错 + 记日志"的最小兼容；想真正裁剪只能在客户端做。
- 放大不存在 ground truth 的"假命中"——只在已检测到命中时把比例托高，避免反向虚报。

### Notes & Caveats
- DB 的 `cached_tokens` 仍写**真实**估算值（不被放大），保留未来重拟合的可用数据；
  只有发给 NewAPI 的 response usage 被放大。
- URL 图片暂不抓取（避免引入 async preprocessing 改造 + 额外延迟），用户需自行 base64。

## [v19] - 2026-05-24
### Features
- **cache_estimate 基线大改：新增 2 个模型 + thinking 变体分离 + 重拟合（n 翻倍）**
  - **新增基线**：
    - `claude-haiku-4-5-20251001`：`(1.42e-6, 39e-6, 0)`，n=26，R²=0.92。
      此前完全无 baseline，所有 haiku 请求 `cached_tokens=NULL` → NewAPI 全价计费。
    - `claude-sonnet-4-6-thinking`：`(2.53e-6, 393e-6, 0)`，n=16，R²=0.89。
      此前同样无 baseline。非 thinking sonnet-4-6 无样本，安全返回 None。
  - **thinking 变体分离**：实测 `claude-*-thinking` 的 b（输出单价）比非 thinking 高 30-60%，
    共享 baseline 在多输出场景显著偏差。各持独立基线：
    - `opus-4-7-thinking`：`(7.22e-6, 406e-6, 0)`，n=53，R²=0.93
    - `opus-4-6-thinking`：`(6.90e-6, 210e-6, 0)`，n=52，R²=0.71
  - **重拟合（样本量翻倍）**：
    - `opus-4-7`：`(7.12e-6, 187e-6, 0.011)` → `(8.37e-6, 296e-6, 0)`，n: 155→381
    - `opus-4-6`：`(6.48e-6, 112e-6, 0)` → `(5.53e-6, 245e-6, 0)`，n: 218→139（cached=0 子集）
    - `sonnet-4-5`：`(6.5e-6, 130e-6, 0)` → `(2.84e-6, 137e-6, 0)`，n: ~10→22。
      **旧 a 高 2× 导致 100% 假命中**，v19 修正后命中率回到 ~5%（短会话本就不易命中）。
  - 全部 c 强制为 0（带 c 拟合常得 c≈0.1 把小请求误判全命中，弃用）。

### Design Rationale
- Kiro 报 `cacheReadInputTokens` 的模型用 `cached_tokens=0` 行作 ground truth；
  不报的模型（opus-4-6 系列、sonnet-4-6-thinking、haiku-4-5）用 `prompt<25k` 作"几乎未命中"代理。
- thinking 变体的 b 显著更高，物理意义清晰：thinking output 含推理 token、单位成本更高。
- sonnet-4-6 仅见 thinking 变体；非 thinking 无样本时返回 None 比"硬塞 thinking 基线"更安全
  （None → NewAPI 全价兜底，最差结果是用户多付；硬塞 → 可能误判命中、少收用户、亏损方向）。

### Notes & Caveats
- 测试 fixture 全部换成真实生产样本（标注 prompt/compl/met），便于后续重拟合时对照判断。
- 后续应每 1-2 月用 DB 重拟合一次：定价漂移、新模型加入。

## [v18] - 2026-05-24
### Features
- **反代访问密钥支持配置多个（任意一个均可通过认证），可在 Admin UI 增删，持久化到 SQLite**：
  - SQLite 新增 `api_keys` 表（`id/key/label/created_at`，`key` 唯一；独立于 `requests`
    环形缓冲，`trim_to` 不会清理）。模块 `src/db/api_keys.rs`。
  - `AppState.api_keys` 为运行时可变的 `Arc<RwLock<Vec<String>>>`（`anthropic::SharedApiKeys`），
    增删 key 后认证路径立即生效，无需重启。认证逐个常量时间比较并累积，不提前返回。
  - 新增 Admin API：`GET /api/admin/api-keys`（列出脱敏 key）、`POST /api/admin/api-keys`
    （新增，重复返回 409）、`DELETE /api/admin/api-keys/{id}`（删除，保留至少 1 个防锁死）。
  - Admin UI 顶栏「API Keys」按钮 + 管理对话框（列表脱敏展示 + 备注 + 增删，`settings-dialog.tsx`）。
  - 启动时以 `api_keys` 表为准；表为空时从 `config.json` 的 `apiKey` 播种 1 条（label=default），
    保证旧部署平滑升级。

### Design Rationale
- 取代 v17 的单 key 方案（`settings` KV 表 + `Arc<RwLock<String>>`）：用户实际需求是多 key。
  v17 已上线但未真正使用单 key 功能，本版直接替换；v17 在 prod 留下的空 `settings` 表为无害孤儿，
  不写 DROP 迁移（避免动 prod）。

### Notes & Caveats
- **删除 key 会立即使用它的客户端失效**（含经此反代出口的 Claude Code 会话）；用"先加新、再删旧"的方式轮换。
- api_keys 写入按需开短连接（WAL + busy_timeout 5s），与 writer 长连接并发安全；写入频率极低。

## [v16] - 2026-05-24
### Fixes
- **opencode(opus-4-6) 缓存计费缺失**：实测 Kiro 对 opus-4-6 不返回 `cacheReadInputTokens`
  （tokenUsageEvent 缺该字段；thinking 不是原因——opus-4-7+thinking 照样有），导致缓存命中也
  按全价计 NewAPI。`cache_estimate` 加 opus-4-6 基线 `(a=6.48e-6, b=112e-6, c=0)`（n=218 拟合
  未命中样本），走估算回退填 `cached_tokens`。大会话命中反推 ~48%、未命中判 0，命中率 ~62%。
- `base.rs` 加非内容事件的 debug 日志，排查不同模型/模式下 Kiro 实际发的计量事件。

### Findings（缓存机制 diff 定位）
- 相邻两轮 `request_body` diff：两个 agent 的 **history 都逐条字节一致**（Claude Code 1144 条、
  opencode 344 条，新轮只追加），conversationId 稳定 —— **代理侧前缀已最优，无"上下文不一致"bug**。
- **Kiro 不按我们发送的 JSON 字节序缓存**：Claude Code 字节 LCP≈0（字段序把每轮变的 currentMessage
  排在 history 前）却仍命中 52% → Kiro 是 parse 后按 history 语义缓存。⇒ 改字节布局的招（cachePoint、
  调字段序、搬 tools）对 Kiro 一律无效，这正是 cachePoint no-op 的根因。
- **命中率 ~52% 是 Kiro 服务端缓存容量上限**：max_cached 随会话增长但比例递减（100k→~70%，
  700k→~58%，封顶约 40 万 token）。客户端无法突破；降本只能缩短会话（产品决策）。

### Notes & Caveats
- **生产事故（已恢复）**：cachePoint 实验期间临时容器挂载了 prod 的 config 目录，导致 prod SQLite
  被替换、writer 进程写已删除的孤儿 inode、新请求静默不落库、可见 DB 从 1633 条掉到 6 条。
  靠 `/proc/<pid>/fd/` dump 孤儿 inode 抢救回全部 1633 条。教训：绝不让临时容器挂载 prod 数据目录。

## [v15] - 2026-05-24
### Features
- Admin UI 请求日志页支持每页条数可选（50/100/200/500，默认 100），1633 条从 33 页降到 17/4 页。
  后端 `limit` 上限 500 不变。

## [v14] - 2026-05-24
### Features
- 接入 `tokenUsageEvent` 解析（`kiro/model/events/token_usage.rs`）：从 Kiro 流式响应拿精确的
  `uncachedInputTokens` / `outputTokens` / `cacheReadInputTokens` / `cacheWriteInputTokens`，
  写入 DB 的 `prompt_tokens` / `completion_tokens` / `cached_tokens` / `cache_creation_tokens`。
  有精确值时跳过启发式缓存估算（`apply_cache_estimate`）。
- 未识别 Kiro 事件类型现会以 info 级打日志（payload 截断 200 字），便于发现需要新增的解析。
- 新增 dormant 实验开关 `KIRO_CACHE_POINT*`：把 Anthropic `cache_control` 翻译为 Kiro `cachePoint`，
  type/config/placement 均可由 env 调，默认关闭。

### Design Rationale
- `tokenUsageEvent` 之前被整体丢弃，导致 `cached_tokens` 永远为空、计费只能靠估算。
  实测它每次请求都会到达，含 thinking 在内的精确 output，计费从此可对齐 Kiro 真实 metering。
- cachePoint 做成 env 可调的实验，是为了用一个二进制扫描多种格式组合，避免反复重编译。

### Notes & Caveats
- **cachePoint 经 A/B 实测为 no-op**：`type=default` 被接受但对缓存命中/metering 零影响
  （冷/热数字与不发 cachePoint 完全一致）；`type=EPHEMERAL/PERSISTENT` 直接 400。
  降低输入的唯一有效杠杆是「稳定 conversationId 触发 Kiro 自动 prefix-cache」，详见 README
  「计费与缓存机制」。该实验代码保留为 dormant，供 Python(ALLinOne) 迁移参考，**勿在 prod 开启**。
- 实测方式：独立可写 DB + 多轮对话 + 唯一 nonce 冷启动，每配置连发 3 次比对
  `prompt/cached/metering`。BASELINE 与 cachePoint(default) 三轮数字逐一相等。
- 部署：musl 交叉编译 → scp → 服务器派生镜像 `local-musl-v14` → `--force-recreate --no-build`，
  容器内 sha256 校验通过。flag OFF 上线，回归行为等同 v13。
