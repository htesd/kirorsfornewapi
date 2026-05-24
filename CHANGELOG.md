# Changelog

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
