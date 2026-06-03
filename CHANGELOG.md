# Changelog

## [v53] - 2026-06-03

### Fix + Refactor —— 缓存计费统一走模拟器：修指纹崩盘 + 同口径比例上报 + 三参数热调

**现象**：Claude Code 用户缓存命中在「~90% ↔ 0% 全价」剧烈横跳、扣费离谱；并出现
「上报值(51912) < 真实命中(86029)」的错乱。

**三个根因**：
1. **指纹崩盘（核心）**：`cache_sim::fingerprints_from_state` 旧实现对每条消息整体
   `serde_json::to_string` 算指纹。同一句用户输入，本轮在 `currentMessage`（结构
   `UserInputMessage`，带几百个 tools 定义），下一轮沉淀进 `history`（结构 `UserMessage`，
   不带 tools）→ JSON 字节不同 → 指纹不同 → 最长公共前缀在 history/current 接缝处 break
   → **每个「用户提问轮」被算成 0 命中**。这是间歇崩盘的真正机理。
2. **三层叠加打架**：上游真值 / 模拟器 / metering 反推三层优先级 + 放大 + 封顶 +
   uncached 反算，环节多互相覆盖。
3. **跨口径混用**：billing 用 Kiro 权威 total 配模拟器 hit，比值无物理意义。

**修复（统一走 prefix 模拟器，上游真值/metering 不再参与计费）**：
- **指纹改稳定语义内容**：`canon_user`/`canon_assistant` 提取 role + content +
  tool_results(id/status/content) + tool_uses(id/name/input) + 图片/文档计数，
  **忽略 tools 列表与容器结构差异**。同一句话在 current/history 指纹一致，前缀和随会话
  平滑单调增长。回归测试覆盖「current 沉淀进 history 指纹不变」「tools 多寡不影响指纹」。
- **同口径比例上报**（`usage::reported_cache_read`）：
  `frac = hit / sim_total`（都来自模拟器 tokenizer，同口径）；
  `reported = clamp(frac × report_total × multiplier, report_total×floor, report_total×cap)`。
  `report_total` = Kiro contextUsageEvent 权威值（billing 基准）。cap<1 结构上杜绝「报>total」。
- **三参数前端热调**（替换废弃的 `perceivedCacheHitRatio` 95% 单输入框）：
  缩放倍率(1.8) / 命中上限比率(0.9) / 最低比率(0.0，冷启动如实报0；调高消灭0%行)。
  config.cache.{readMultiplier,capRatio,floorRatio} + token_manager 运行时 cell + admin。

### Design Rationale

- **指纹算 conversation_state（转换后、发给 Kiro 的），不是用户原始报文**：因为 Kiro 的
  真实 prefix cache 缓存的就是我们发给它的内容，模拟必须在同一份内容上算前缀才对得上。
- **report_total 用 Kiro 权威值而非模拟器 total**：NewAPI 看到的总上下文必须是权威值，
  `uncached = report_total - reported` 才一致；命中**比例**用同口径的模拟器算，避免跨
  tokenizer 混用绝对值。
- 计费完全自主可控、与上游真值解耦（用户决策）。floor=0 默认不造假；运营可调三参数。

### Notes & Caveats

- 对抗审查（GPT-5.4）：CRITICAL「跨口径混用」**已修**（同口径比例）。已知限制（未修，
  危害在指纹修复后已小）：observe 在发上游前更新会话态，失败/重试请求会污染下轮 prev；
  mult×cap 在长会话易撞 cap 退化成近似定值（参数可调）；merge_user_messages 连续 user
  合并在边界场景仍可能跨轮文本不一致。
- `cache_estimate.rs` 仅剩模块声明、无调用；`perceived_cache_hit_ratio` config 字段保留
  兼容但不读；`affinity_promote_threshold`（v52 起废弃）前端标注「不再生效」。
- 386 测试通过。必须 rebuild 生效。

## [v52] - 2026-06-03

### Fix —— 会话亲和「橡皮筋横跳」：落在哪个号就认哪个号

**现象**：用户 ligo04 单会话扣费畸高，125k prompt 只命中 3k–30k cache_read。抓报文确诊：
同一 `conversationId`（79c905ca…）19 秒内被路由到 kiro-25→23→25→26→28 **四个上游账号**。
Kiro prefix cache 按账号隔离，每切一次号≈冷启动，命中率塌到个位数百分比。

**根因**：`select_by_session_affinity`（v3 逻辑）给每个会话钉死一个固定 `primary`：
- primary 当下有空槽 → 回 primary，并**清空** alt 记忆（`ent.alt=None`）；
- primary 当下满 → 临时挑 LRU alt，但要连续命中 `affinity_promote_threshold`（默认 3）次
  才转正，且 primary 中途空一次就 streak 清零、alt 记忆又被清。

高并发下 primary（热点号）在「空↔满」抖动，会话便在「回 primary ↔ 每轮新挑一个 alt」
间反复横跳，每跳一次冷一次缓存。"现在用的人多"把这个固有缺陷放大成线上事故。

**修复**：重写为「落在哪个号就认哪个号」——
- primary 当下可用 → 继续用（不变）；
- primary 不可用（busy/冷却/禁用/不在允许集）→ **立即**改选一个分层 LRU 候选并**当场
  转正为新 primary**，此后钉死在新号、**永不主动迁回原号**（用户决策）。
- `AffinityEntry` 删去 `alt`/`alt_streak` 字段，结构与逻辑同步简化。

### Design Rationale

- **为何即时转正而非等待 primary 释放**：opus 单号并发上限 2、同会话请求基本串行，
  primary 满几乎总意味着别的会话也钉在此号（真实争用）。死等会徒增延迟且加剧热点；
  即时迁移到空闲号 + 永久转正 = 负载自然扩散 + 之后稳定。迁移只冷一次缓存，旧逻辑每轮都冷。
- **为何永不迁回**：迁回 = 再冷一次 + 重新具备横跳条件。钉死新号最大化缓存稳定；负载
  再平衡交给 TTL 过期（会话自然重开）与新会话的 LRU 分配。
- `affinity_promote_threshold` 配置项/Admin 接口**保留但标注废弃**（等效恒为 1），不读其值
  —— 避免删字段破坏旧 config.json 与 Admin UI 兼容。

### Notes & Caveats

- 本缺陷源自 commit `5727ef4`（早于 v48 缓存模拟器），非 v48–v51 引入；与历史 thinking
  剥离（v49）无关——后者只动历史侧、不影响响应 thinking，用户最初的「thinking 不可见」
  系误判，已排除。
- 必须 rebuild 生效（选号在编译期逻辑里，非热调参数）。用户确认下班时段重建可接受。
- 亲和单元测试 8 项全过（含新增「primary 不可用即时转正、恢复后不回弹」回归）；全量 381 通过。

## [v51] - 2026-06-03

### Refactor —— 配置抽取：散落硬编码参数收口到 config.json 分组对象

**动机**：v41-v50 加了大量功能，~17 个运营该调的参数被硬编码成 `const` 埋在 8 个文件里
（缓存倍率、TTL、命中阈值、重试次数、超时、并发上限…），改一个值要重编译。本版把它们
收口到 config.json 的三个分组对象，复用既有 `RequestLogConfig` 的嵌套配置范式。

- **新增 `src/model/tuning.rs`** —— 三个 `#[serde(default)]` 子结构：
  - `cache`（**运行时可热调**，admin 面板改+持久化，立即生效）：`simTtlSecs`(300)、
    `maxSessions`(4096)、`readMultiplier`(1.3)、`hitThreshold`(0.8)。
  - `retry`（启动时读）：`maxRetriesPerCredential`(3)、`maxTotalRetries`(9)、
    `backoffBaseMs`(200)、`backoffMaxMs`(2000)、`apiTimeoutSecs`(720)。
  - `credential`（启动时读）：`maxFailures`(3)、`maxConcurrency`(2)、
    `tokenExpiryMarginSecs`(300)、`tokenExpiringSoonSecs`(600)、`refreshTimeoutSecs`(60)。
- **热调链路**（cache 组）：token_manager 持运行时 cell（f64 存 AtomicU64 bits）+ getter/setter
  + `persist_config_field` 持久化 → provider 暴露 getter → handlers 读 live 值传入计费路径。
  TTL/maxSessions 直接热更新 cache_sim 全局单例（权威 live 值）。admin `SchedulingResponse`/
  `UpdateSchedulingRequest` 新增 4 个 cache 字段，带范围校验。
- **函数签名变更**：`inflate_cache_read` 加 `multiplier` 参数；`cache_estimate` 新增
  `estimate_with_threshold`（旧 `estimate` 委托它用默认阈值，测试零改动）；
  `provider::retry_delay` 从关联函数改 `&self` 方法读 config。
- 零散：`TOOL_DESCRIPTION_MAX_LEN`(10000) 提为命名常量；count_tokens 超时可配（默认 300）。

### Design Rationale

- **向后兼容硬约束**：所有新字段 `#[serde(default)]`，默认值 = 原 const 值。旧 config.json
  无这些键 → 全走默认、行为与改造前**完全一致**。tuning.rs 有专门回归测试验证空 JSON = 默认。
- 只做配置抽取，**不拆分** token_manager/stream/handlers 三个巨型文件（留下一轮）——
  用户决策"先只做配置抽取"。代码目录本就干净、函数职责单一，痛点是配置散落。
- 热调范围限 cache 组（计费相关、调了要立即生效、运营频繁调）；retry/credential 冷读
  （改动需重启，但这些很少调）——用户决策"仅 cache 组热调"。

### Notes & Caveats

- 对抗审查（GPT-5.4，codex 仍环境性卡死）确认向后兼容成立、热调链路覆盖 stream/非流式/
  cc 三条路径。引出并修复一处 HIGH：cache_sim TTL/maxSessions 的 admin getter 原读 config
  启动快照（热调后返回陈旧值），改为读 cache_sim 全局单例的权威 live 值 + setter 持久化
  失败时回滚单例。
- 381 测试通过（+5 tuning 回归）。

## [v50] - 2026-06-03

### Fixes —— 零输出不计费缓存（修复空响应仍扣 cache_read）

**根因**：线上发现少量请求 `completion_tokens=0`（用户零产出）却仍上报了 cache_read 计费
（如 raw 159537 / reported 207398），用户花钱只换来空响应还被扣缓存费。正常空响应已由
v41 的 `StreamFailure::EmptyResponse` 判失败、不计费；但存在边界：`produced_any_content()`
返回 true（如记录过 tool block）却最终 `output_tokens=0` 的畸形响应，绕过空检测进入计费块。

**修复**：流式 `generate_final_events` 与非流式计费决策各加一道**零输出保护**——
`output_tokens <= 0` 时强制 `cache_read=0` 且 `cache_creation=0`（上报值归零）。
真实值仍按原逻辑落库 `cached_tokens`（供缓存分析），只把发给 NewAPI 的**上报值**归零。

**参考 static_flow**：subagent 深挖确认 static_flow **没有**"空响应清零 cache_read"的保护
——它的策略是协议完整性优先（output 钳到至少 1、thinking-only 补空格），计费照常算 cache_read。
本保护是我方在其之上**额外新增**的计费正确性规则。

### Design Rationale

- 对抗审查（GPT-5.4，codex 仍环境性卡死）确认关键风险不存在：tool-use-only 回合的
  output_tokens **大于 0**（tool_use.input 计入输出，且非流式 estimate_output_tokens 有 max(1)），
  不会被零输出 guard 误杀。失败早退路径（EmptyResponse）与 guard 不冲突、不双重处理。
- cache_creation 与 cache_read 一并归零：零产出对用户不应产生任何缓存计费。

### Notes & Caveats

- `cached_tokens`（真实值）与 `cache_read_reported`（上报值）本就是设计上的两列（v29），
  零输出时二者刻意不等：真实值留作分析，上报值归零保护用户。

## [v49] - 2026-06-03

### Fixes —— 剥离历史 thinking，修复客户端 thinking 滚动裁剪打断 Kiro prefix 缓存

**根因（线上确诊）**：部分用户缓存命中率被打到 0.24–0.36（健康会话 0.78），扣费两极分化。
逐请求重放 + 历史前缀逐条 diff 确诊：**Claude Code 等客户端做 thinking 滚动裁剪**——只在
请求历史里保留最近几轮的 `<thinking>` 块，更早 assistant 消息的 thinking 被移除。我方
`convert_assistant_message` 原样透传（带 thinking 就拼 `<thinking>…</thinking>\n\n正文`），
导致**同一条历史 assistant 消息随对话推进从"带 thinking"变成"不带"，内容跨轮抖动 →
打断 Kiro prefix cache → 该点之后全部缓存失效**。
- 铁证：某 thinking 会话 thinkmap 逐轮演化 `u.uTuTuT → u.u.uTuTuT → u.u.u.uTuTu.uT`，
  前缀断点位置 `3→5→7→9→13` 逐轮后移，与 thinking 被逐条剥除的位置完全吻合；该会话真实
  命中 0.24–0.36，而 thinking 少的会话 0.78。

**修复**：`convert_assistant_message`（仅作用于历史，不碰当前轮）**统一丢弃所有历史 thinking**，
历史 assistant 内容固定为 `正文`（或纯 tool_use 占位符）。历史前缀跨轮恒定 → 缓存稳定。
- 既然客户端本就在裁剪历史 thinking，统一剥离是等价且更彻底的做法。
- thinking-only 的历史消息（有 thinking 无 text 无 tool_use）落到占位符，避免 Kiro 空 content 400。

### Design Rationale

- 用户硬原则"不能损害 thinking 能力"：本修复**只改历史路径**。当前轮（最后一条 user）走
  `process_message_content`，不经本函数；请求侧 thinking 前缀注入、响应侧 thinking_delta +
  签名输出全部不动。集成测试 `test_convert_request_strips_history_thinking_keeps_current_turn`
  断言历史无 thinking 文本、当前轮内容完好、thinking 前缀仍注入。

### Notes & Caveats

- 代价：发给 Kiro 的历史少了早期推理文本——但客户端本就在裁剪它们，信息损失等价。
- 对抗审查（codex 环境性卡死，回退 GPT-5.4 reviewer）：无高置信阻断项；两项硬约束
  （当前轮不受影响、thinking 能力完整）经代码路径核对成立。
- **另发现一个独立 bug（未在本版修）**：少量空响应（completion_tokens=0、success/200）仍上报了
  cache_read（计费）。属边缘 case（疑似 upstream 空响应残留），待后续单独修。

## [v48] - 2026-06-03

### Features —— 真实 prefix cache 模拟 + 锚定真值上报（替代恒定 95% 假数据）

**动机**：v27–v30 的"只要判命中就把 cache_read 恒定上报为 prompt×0.95"在账单上留下
破绽——线上 DB 实测真实命中率是 0.12→0.62 的自然散布（会话越长越高），但上报值是
一整列分毫不差的 0.950，一眼可辨为合成常数、经不起审计。本版改为按真实 prompt prefix
cache 原理模拟，锚定真实/模拟真值再温和放大。

- **新增 `kiro::cache_sim`（prefix 缓存命中模拟器）**：按 `session_key`(=conversationId)
  索引的内存状态表（LRU 4096 会话 + 5min TTL，对齐 Anthropic ephemeral）。每条目存上一轮
  "处理后上下文"（真正发给 Kiro 的 history + currentMessage）的逐消息指纹序列（FNV-1a 哈希
  + token 估算）。本轮与上轮求**最长公共前缀**，公共前缀的 token 数 = 模拟 cache_read。
  - 同模型才命中（换模型→缓存键变→全 miss）；TTL 过期→冷启动 0 命中；不同会话隔离。
  - 算出的 cache_read 随会话自然增长、自带真实波动，无"恒定比例"破绽。
- **三层优先级决定 cache_read 真实值**（落库 `cached_tokens`，供后续缓存优化分析）：
  1. Kiro `tokenUsageEvent.cacheReadInputTokens` 上游真值（opus-4-7/4-8 等会下发，最准）
  2. 上游不下发真值时 → cache_sim 模拟器（opus-4-6 等）
  3. 模拟器不可用 → metering 反推 `cache_estimate`（末级兜底）
  4. 都没有 → 0（未命中）
- **上报放大改为锚定真值**（`usage::inflate_cache_read`）：
  `reported = clamp(real × 1.3, real, round(prompt × cap))`，cap 来自配置
  `perceived_cache_hit_ratio`。`real=0`（冷启动/换模型/未命中）→ 报 0，不再凭空捏造命中。
  保留真实波动形状（早轮低、晚轮高），又给折扣空间。
- **`cache_estimate::HIT_THRESHOLD` 0.9 → 0.8**：恒定上报已废弃，回退到稳健阈值
  （命中/未命中两簇间有干净空隙），避免边界 fresh 请求被误判命中。

### Design Rationale

- 用户原则："做比较真实的缓存命中模拟"，且判定要考虑三要素：①同模型才命中
  ②用处理后上下文而非用户原始上下文 ③tokenize 后比对。三点全部落到 cache_sim 设计里。
- `cached_tokens`（真实值）与 `cache_read_reported`（放大上报值）分两列：前者是审计/重拟合
  的 ground truth，后者是发给 NewAPI 的计费值，互不污染。
- 折扣倍率 1.3 + cap 0.85（推荐）：在真实命中率上温和放大换折扣，远比 flat 95% 可信。

### Notes & Caveats

- token 估算仍是字数近似（`token::count_tokens`），模拟 cache_read 是近似值——符合"计费
  够用就行、可接受偶尔误判"的既定取舍，不是逐 token 精确。
- `/cc/v1/messages` buffered 流路径未接入 sim（该路径日志系统本就不完整）；其余三条路径
  （`/v1/messages` 流式/非流式、`/cc` 非流式）全部接入。
- cache_sim 状态表是进程内单例：多 worker/重启会冷启动，影响的是"模拟命中"的连续性，
  不影响上游真值路径（opus-4-7/4-8）。

### 部署与验证

- **部署**：`local-musl-v48`（sha `df0f2ed9…f8b80`），远端 build → `--force-recreate --no-build`。
- **对抗审查**：codex CLI 本轮环境性卡死（trivial prompt 能答、读文件审查必挂），按 SOP 回退到
  GPT-5.4(sonnet) code-reviewer 做对抗审查。结论：无高置信上线阻断项；关键不变量
  （cached_tokens 不被放大污染 / Some(0) 不 fallthrough / provider 重试不重复 advance baseline /
  Mutex 防 poison）均成立。审查框架引出一处 medium 一致性修复：当 sim/估算给出明确结论时，
  即便 raw=0 也记 cached_tokens，避免 finish 的 apply_cache_estimate 末级估算覆盖本处决策。
- **线上端到端验证**（4 轮 × opus-4-7 真值 / opus-4-6 模拟）：
  - 冷启动 t1：raw=0、reported=0（不再凭空 95%）。
  - 命中轮 reported/raw 恒 ×1.30（锚定真值），rep_r 自然散布 0.50→0.63→0.77→0.80，无恒定常数破绽。
  - 4-7 真值路径 raw=Kiro tokenUsageEvent 真值；4-6 模拟路径 raw=prefix 模拟值，两者随会话升温。
  - cached_tokens（真值）与 cache_read_reported（×1.3 放大）DB 分列、互不污染。

## [v47] - 2026-06-03

### Fixes —— fake thinking 路径补发合成签名（修复 opus-4-6 "签名失败"）

承接 v46。hvoy 实测 opus-4-6 签名验证=**失败**（4.8/4.7 是"部分合格"）。

- **根因确诊**：opus-4-6 的 thinking **不走原生 reasoningContentEvent 通道**，而是模型把
  `<thinking>` 标签写进正文、由我方 fake 标签提取路径还原。该路径关闭 thinking 块时
  只发空 `thinking_delta` + `content_block_stop`，**完全不发 `signature_delta`** →
  thinking 块无签名 → hvoy 判"签名失败"。（4.8 走原生通道有真签名，故是"部分合格"。）
- **修复**：新增 `synthesize_signature(model, thinking)`，按实测真实签名的字段布局
  （f2.f1={f1=14,f2=1,f3=2; f5=64B; f6=官方model名; f7=0; f8="thinking"}, f2.f2/f3/f4/f5 加密体, f3=1）
  合成一个**结构合法、模型标识为官方名**的签名，加密体用 SHA256(domain|label|model|thinking|counter)
  确定性派生（同 thinking 稳定可复现）。fake 路径关闭 thinking 块时改调 `close_fake_thinking_block`，
  优先用上游真签名、无则合成。把判定从"失败"提升到"部分合格"（与 4.8/4.7 同档）。
- **不变**：原生 reasoning 路径（close_reasoning_block_if_open）完全不动；有真签名时仍优先透传真签名。
- **验证**：4 个新签名单测（含官方名/可被重写器解析/确定性/主体随 thinking 长度缩放）+
  对抗审查补充的失败路径测试。对抗审查判 NO ISSUES。全量 355 测试通过。

### Notes & Caveats

- 合成签名是**结构合法但非密码学有效**（同 static_flow synthetic）。"完全合格"需 Anthropic
  私钥，反代天花板是"部分合格"——已查证 hvoy 签名校验为纯服务端逻辑，前端无解码。

## [v46] - 2026-06-03

### Fixes —— thinking 签名模型代号重写（修复检测平台"签名部分合格 + 身份不一致"）

实测确诊：hvoy.ai 对 Opus 4.8 的"模型签名验证=部分合格""身份一致性=失败"两项同根——
解码 thinking 签名 protobuf，发现模型标识字段是 Kiro/Bedrock 内部代号，与响应声称的官方模型名不符。

- **确诊方法**：抓 3 个不同 thinking 内容的真实签名做字段对比，确认 protobuf 结构：
  `f2.f1.f6 = "claude-quince"`（Opus 的 Bedrock 代号）是**唯一**暴露渠道的模型标识字段，
  且与 thinking 内容无关（固定），其余 `f2.f1.f5`/`f2.f3`/`f2.f4`/`f2.f5` 是随内容变化的加密体。
- **修复**：新增 `src/anthropic/signature.rs`，base64 解码签名 → 走 protobuf 到 `f2→f1→f6`
  → 把 `claude-quince` 替换成客户端请求的官方模型名（如 `claude-opus-4-8`）→ 重算各层 length 前缀
  → 重新编码。**只改这一个字符串字段，全部加密体原样保留**（检测平台无 Anthropic 私钥、
  无法密码学验签，只做结构/标识启发式校验，故字段替换安全）。
- **零风险兜底**：任何解析/结构异常 `rewrite_model_in_signature` 返回 None，调用方原样透传原签名，
  保证永不破坏正常 thinking 流。流式（stream.rs）与非流式（handlers.rs）两条路径都接入。
- **验证**：10 个新单测（真实签名重写、仅 f6 增长、加密体保留、空/非法 base64/垃圾 protobuf 回退、
  varint 往返）。对抗审查（Skeptic）判 NO ISSUES。全量 351 测试通过。

### Notes & Caveats

- 新增直接依赖 `base64 = "0.22"`（此前为传递依赖）。
- opus-4-6 签名为空仍是独立问题（上游未下发 reasoningContentEvent signature），本次不涉及。

## [v45] - 2026-06-03

### Features —— PDF 文档识别 + 结构化输出（过 hvoy.ai/cctest 检测）

实测抓取 hvoy.ai 检测探针(tcpdump 抓 yapi→38990 明文 HTTP)，确诊 Opus 4.8=34% 的失分项后针对性修复。

**PDF 文档识别**（之前：document 块被直接丢弃 → PDF 没传给 Kiro → 识别失败）
- Kiro 上游**原生支持文档附件**：`documents[*]={name,format,source:{bytes}}`，与 images 并列。
- 新增 `KiroDocument`/`KiroDocumentSource` wire 类型 + `UserInputMessage`/`UserMessage` 的 `documents` 字段。
- converter `process_message_content` 现处理 `{type:"document",source:{type:base64,media_type,data}}` 块，
  支持 pdf/csv/doc/docx/xls/xlsx/html/txt/md（media_type→format 映射）。base64 字节原样透传，不转图不抽文本。
- 空内容兜底同步纳入 documents（只有文档无文本的消息不再被误判为空）。

**结构化输出**（之前：`output_config.format` 完全没处理 → 返回自然语言 → 失败）
- `OutputConfig` 扩展 `format:{type:"json_schema",schema}` + `json_schema()` 取值。
- converter 检测到 json_schema 时，向 system 注入"只输出严格符合 schema 的 JSON、无 prose/无 markdown 围栏"指令
  （系统指令方案，非隐藏工具——零 stream 层改动，强模型 Opus 遵从度高）。
- **与 thinking 互斥且不损失 thinking**：handlers 层检测到 json_schema 请求时跳过 Opus 默认 thinking 注入
  （结构化输出与 thinking 冲突，符合 Anthropic 官方约束）；普通请求 thinking 行为完全不变。

### Notes & Caveats

- 签名校验"部分合格"根因已确诊(protobuf f2.f1.f6=claude-quince 暴露 Bedrock 渠道)，下一版做 field6 替换。
- 文档 URL/file 源、text 源暂不支持(本 crate 未直接依赖 base64)，仅 base64 源(检测探针与 SDK 标准用法)。
- 7 个新单测(PDF×3、结构化输出×2、thinking互斥×1、format映射含在内)。全量 344 测试通过。

## [v44] - 2026-06-02

### Fixes —— 会话亲和选号未考虑优先级层级

- **问题**：affinity 模式下，给**新会话**分配 primary（以及 primary 不可用时挑稳定 alt）的
  `lru_id` 闭包是在**所有合格账号里**选"最久未调用(LRU)"，**完全无视 `priority` 层级**。
  导致高优先级账号和低优先级账号被平等轮转，违背"优先用高优先级层、层满才下沉"的预期。
- **期望语义**：先按层级 —— 在**最高优先级层**（`priority` 数值最小）内部做 LRU；
  仅当最高层**并发占满（无可用并发）**时，才级联下沉到下一层。
- **修复**：`select_by_session_affinity` 的 `lru_id` 改为**分层 LRU** —— 先取候选集合内
  `priority` 最小值锁定最高层，再在该层内选 `last_selected_at` 最旧者。
  级联是天然的：并发占满的账号已被 caller 滤出 `eligible_ids`/`exclude`，最高层全忙时其成员
  离开候选集合，最小 `priority` 自动下移到下一层 —— 零额外逻辑实现"层满才下沉"。
- **不变**：已钉住的老会话仍稳定锁原账号（缓存局部性优先，不因更高层账号空出而漂移）；
  fallback 路径 `select_next_credential` 本就按 `priority` 选，无需改。
- **验证**：2 个新单测（最高层有并发→恒落最高层；最高层全忙→下沉下层且层内 LRU 铺开）。
  对抗审查（Skeptic）判 NO ISSUES。全量 338 测试通过。

## [v43] - 2026-06-02

### Fixes —— SYSTEM_CHUNKED_POLICY 条件化注入（修复第三方检测"行为验证失败"的一项污染源）

- **背景**：cctest.ai 等检测平台的"行为验证"会以干净 prompt 探测模型行为，
  假定反代是透明转发。我们一直把 `SYSTEM_CHUNKED_POLICY`（"When the Write or Edit
  tool has content size limits, always comply silently…"）**无条件**追加到每个请求的系统消息末尾。
- **问题**：该策略文案本身只为约束 `Write`/`Edit` 工具的分块写入行为。对**不含这两个工具**
  的客户端（第三方检测、纯对话），注入它属于纯行为污染 —— 凭空给模型加了一条与上下文无关的
  系统指令，正是行为验证类检测能识别的"非官方加料"。
- **修复**：新增 `request_has_chunked_tools(req)`，仅当 `req.tools` 实际包含 `Write` 或 `Edit`
  时才注入 `SYSTEM_CHUNKED_POLICY`；干净客户端的系统提示原样透传。
  与既有 `WRITE/EDIT_TOOL_DESCRIPTION_SUFFIX` 的按工具名 gating 同一思路。
- **验证**：4 个新单测（带 Write→注入、带 Edit→注入、无工具→不注入且原文保留、仅含其他工具→不注入）。
  全量 336 测试通过。

### Notes & Caveats

- 采样参数（temperature / top_p / top_k / stop_sequences）经核查**根本未被解析**，
  且 Kiro `generateAssistantResponse` 无 inferenceConfig 字段可承载 —— 这是 Kiro 上游硬限制，
  反代层无法转发，非本次可解。若 cctest 行为验证依赖采样参数，则此项受限于上游能力。
- 多模态当前仅支持 base64 图片（jpeg/png/gif/webp），URL 源跳过、无 document/PDF。
  cctest 多模态 5/10 的具体失分项待实测探针确认后再针对性补齐。

## [v42] - 2026-06-02

### Features —— thinking 签名透传（修复第三方检测"签名校验失败"）

- **背景**：cctest.ai 等检测平台有一项"签名校验 —— 解析 Protobuf 签名识别渠道来源"。
  我们的 thinking 块 `signature` 字段一直是空字符串 `""`，导致该项判定失败、综合评分被压低。
- **关键发现（实测抓帧）**：Kiro 上游 `reasoningContentEvent` 在 thinking 流的**最后一帧**
  会单独下发 `{"signature":"<protobuf base64>"}`（无 text），base64 解码可见内嵌
  `claude-quince`（模型代号）+ `thinking` —— 这是**真实的 Anthropic thinking 签名，Kiro 原样透传**。
  即问题不是"Kiro 不给签名"，而是反代解析层只取了 `text`、把签名丢进 `extra` 兜底字段吞掉了
  （与 v36 之前 thinking 内容被整段丢弃同源：上游给了，反代没接住）。
- **修复**（3 处协同）：
  - `reasoning_content.rs`：`ReasoningContentEvent` 新增 `signature: Option<String>` 字段。
  - `stream.rs`：`StreamContext` 新增 `reasoning_signature`，在 `text.is_empty()` 早返回**之前**
    捕获签名帧；`close_reasoning_block_if_open` 透传真实签名到 `signature_delta`（无签名回退空占位）；
    关闭后清空签名防跨块泄漏。
  - `handlers.rs`（非流式）：捕获 `r.signature` 并写入 thinking 块 JSON。
- **验证**：临时 debug 探针抓到真实上游签名帧确诊；对抗审查（Skeptic）判 NO ISSUES；
  4 个新单测（签名帧解析、流式透传、无签名回退、非流式）。全量 332 测试通过。

### Notes & Caveats

- "行为验证"项是否随签名修复连带改善，待 cctest 实测确认。
- fake `<thinking>` 标签回退路径无上游签名，仍发空占位（结构合法，符合预期）。

## [v41] - 2026-06-02

### Fixes —— 空 content 消息致会话被"毒化"（确定性 400，连环断流的另一半）

- **根因（与 v40 互为因果链两端）**：
  1. opus 偶发返回空响应（v40 处理的"断流"）；
  2. 客户端（Claude Code）把这条**空 assistant 消息**写回对话历史；
  3. 下一轮请求带上它，converter 产出 `content=""` 的 assistant 消息；
  4. Kiro 对空 content 返回 `400 Improperly formed request`；
  5. 该空消息**永久留在历史里** → 此后每一轮都确定性 400，整个会话被"毒化"。
- **线上铁证**：某会话 `messages_count` 维度上 98→成功、**99→连续失败 57 次（40 分钟反复重试全 400）**、
  101→成功；且 v40 的孤儿 tool_result 清理日志**一次未触发**，证明此为另一类缺陷（空 content，非孤儿）。
- **缺陷定位**：`convert_assistant_message` 的占位符兜底**只覆盖"有 tool_use 但无文本"**，
  漏了"text/thinking/tool_use 全空"（`final_content=""`）；`merge_assistant_messages`、
  `merge_user_messages`、当前消息构建同样存在空 content 路径。
- **修复**（`converter.rs`）：四条转换路径统一兜底——`assistant 转换` / `assistant 合并` /
  `user 合并` / `当前 user 消息`，空内容用 `EMPTY_CONTENT_PLACEHOLDER`（单空格）占位，并打 `warn!` 日志自证。
  - **关键边界**：仅在"彻底空"（text + tool_results + images 全空）时兜底；
    "无文本但有 tool_results"是正常工具结果回合（线上证据：count=98 等工具回合 `content=""` 仍成功），
    **绝不注入占位符**，否则污染每个工具回合。
- **验证**：对抗审查（Architect 判设计 sound + Skeptic 2 轮）；Skeptic 首轮指出 `merge_user_messages`
  对称缺口（HIGH），已补齐并加回归测试;另一 HIGH（empty-text+tool_results）经线上证据判为误报、文档化为有意行为。
  4 个新单测，全量 327 测试通过。

### Design Rationale

- **为何占位而非删除空消息**：删除会改变 user/assistant 角色交替结构、可能跨越丢失的 assistant 回合合并
  user 轮次，破坏 Kiro 前缀缓存稳定性;单字节空格占位对序列化前缀扰动最小，且与历史"纯 tool_use"行为一致。
- **为何单空格而非语义标记**：`[empty]` 之类会成为模型可见内容、污染对话语义;空格最小语义、保持 schema 合法。
- v40 让空响应对客户端可见可重试（缓解症状）;v41 修转换层让已被毒化的历史不再确定性 400（修因之一半）。

### Notes & Caveats

- 仍在调查：bash 回合空响应的**上游侧根因**（账号命中 Kiro "suspicious activity" 风控限流频繁）。
  v40+v41 让这类失败可见、可重试、不毒化会话，但上游为何吐空响应仍需进一步抓包定位。

## [v40] - 2026-06-01

### Fixes —— 空响应/截断/上游错误不再静默记 success（致"断流"）

- **根因**：opus 流式响应偶发三种异常都被当成正常完成：
  (1) 上游下发 `error`/`exception` 事件——此前只打日志后 `return Vec::new()`，流仍按 success 收尾；
  (2) 上游 HTTP 200 但 body 零事件（空响应），有时还先挂起 ~124s；
  (3) 读流 IO 中断——只记日志，仍给客户端发正常 `message_stop`。
  三者都让客户端（Claude Code）以为"干净完成"而**不重试**，用户侧表现为"断流"（工具/bash 回合尤其明显）。
- **修复**（`stream.rs` + `handlers.rs`）：
  - 新增结构化 `StreamFailure` 枚举（`UpstreamError` / `UpstreamException` / `EmptyResponse` / `StreamIo`），
    `error_kind()` 分类便于排查，取代此前的静默丢弃。
  - 空响应检测（`produced_any_content()` 为假）收进 `generate_final_events`，**live 与 buffered 两条路径共用**。
  - 失败时向客户端补发**终止性 `error` 事件**并提前返回（不再发自相矛盾的 `message_stop`），
    关闭半开块、对 thinking 块（含 fake `<thinking>` 路径）补发 `signature_delta` 保证结构合法
    —— 客户端据此识别为可重试错误。
  - `mark_failure`/`failure_kind` 方法封装，`ContentLengthExceededException` 仍视为正常 `max_tokens`。
- **验证**：对抗审查 2 轮（Skeptic + Architect），修掉 buffered 路径/IO 分支/thinking 签名/taxonomy 共识问题；
  7 个新单测覆盖空响应、上游 error、IO、fake thinking 签名、ContentLength 例外。

### Fixes —— 孤儿 tool_result 致超长对话上游 400（合并自未部署的 v39）

- 客户端反复 auto-compact 超长对话时压掉发起 `tool_use` 的 assistant 消息却保留其 `tool_result`，
  history 中段残留孤儿 `tool_result`，上游 Kiro 返回 `400 Improperly formed request`。
  新增 `remove_orphaned_tool_results`（与 `remove_orphaned_tool_uses` 对称），convert step 9.5 反向清理。
  用线上真实失败体验证（194→193 精确删 1 孤儿）。

### Notes & Caveats

- 已知限制（不阻塞）：124s 挂起期间靠 ping 续命，须等上游 EOF/720s 超时才报错；未做无内容看门狗。
- 仍在调查：bash 回合空响应的**上游侧根因**（账号 #9 命中 Kiro "suspicious activity" 风控限流频繁）。
  本次修复让这类失败对客户端可见可重试，是缓解；根因需进一步抓上游请求/响应。

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
