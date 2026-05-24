# Changelog

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
