# Kiro 后端 prompt cache 行为实验结果（2026-05-23）

## TL;DR

**有 cache，命中后 metering 降 65%**。前提是 `conversationId` 和 `agentContinuationId` 在同会话内都稳定。两个 ID 都用内容指纹派生后，连续相同请求 5/6 次命中 cache（usage ~0.038），1/6 次 miss（usage ~0.109）。

## 已部署的关键修复（v4 + v5）

### v3 → v4：剥掉 `x-anthropic-billing-header`

Claude Code 客户端把一行 `x-anthropic-billing-header: cc_version=...; cc_entrypoint=cli; cch=<5位hex>;` 拼进 system prompt。`cch` 是 Anthropic 的 attribution token（基于整个 body 的 xxHash64，每请求都不同）。

- 对 Kiro 链路：**完全无意义**，Kiro 不读这个 token
- 占用 ~80 token 输入白花钱
- v4 在 `converter.rs::strip_rolling_fingerprints` 把以 `x-anthropic-` 开头的整行剥掉

### v4 → v5：`agentContinuationId` 也用内容指纹

```rust
// 之前
let agent_continuation_id = Uuid::new_v4().to_string();
// 之后
let agent_continuation_id = derive_agent_continuation_id(&conversation_id);
```

这是 cache 命中的关键。同 conversationId 但每次新 agentContinuationId 时，Kiro 后端 cache miss、metering 3 倍。两个 ID 都稳定后立刻命中 cache。

## 实验数据（v5，2026-05-23 03:51）

发 6 次完全相同的 payload，间隔 3 秒：

```
prompt   completion  usage    ctx_pct
6712     55          0.0379   0.67    ← hit
6714     51          0.0381   0.67    ← hit
6746     29          0.0459   0.67    ← hit（output 短，usage 也短）
6714     52          0.0381   0.67    ← hit
7006     27          0.1093   0.70    ← MISS（usage 涨 2.9 倍）
6693     28          0.0330   0.67    ← hit
```

Body canonical hash（除 agentContinuationId 外）完全相同，但 Kiro 后端 prompt_tokens（从 contextUsageEvent 来）和 metering 偶尔抖动。

## 还没解释的现象

1. **完全相同 body 偶尔 cache miss** — 怀疑：Kiro 后端 cache 有 TTL/容量驱逐，或有多实例路由抖动。需要更多数据验证
2. **prompt_tokens 6712 vs 7006 都来自 Kiro 后端的 contextUsageEvent** — 不是我们估算的，是 Kiro 自己报的值
3. **Kiro 后端 cache 持久性** — v4 实验留下的 cache 4 分钟后仍生效

## 一些数字（v5 实测）

- **cache 命中折扣**：~65%（0.038 vs 0.109）
- **同会话连续 turn 平均节省**：约 60-70%
- **agentContinuationId 稳定的影响**：从 9 次连续全 miss → 5/6 次命中

## 下一步建议（醒来再决定）

1. 把 cache hit 反推为 Anthropic `cache_read_input_tokens` 字段塞进 usage 返回给 NewAPI
   - 公式：`cache_read ≈ prompt_tokens × (1 − 实际_metering / 预期_无cache_metering)`
   - 需要先拟合"无 cache 单价基线"
2. 修流式 `completion_tokens`（V2 遗留）
3. 多发几小时数据确认 cache miss 偶发率，决定要不要做重试机制

## 相关文件

- `src/anthropic/converter.rs::strip_rolling_fingerprints` — 剥 billing header
- `src/anthropic/converter.rs::derive_conversation_id_from_messages` — conversationId 内容指纹
- `src/anthropic/converter.rs::derive_agent_continuation_id` — agentContinuationId 派生
- `src/anthropic/handlers.rs:677` — context_usage_pct 转 prompt_tokens 的位置
