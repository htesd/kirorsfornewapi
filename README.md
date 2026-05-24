# kiro-rs

一个用 Rust 编写的 Anthropic Claude API 兼容代理服务，将 Anthropic API 请求转换为 Kiro API 请求。

> ## 关于本 Fork (`htesd/kirorsfornewapi`)
>
> 本仓库在 upstream `hank9999/kiro.rs` 基础上做了几项面向 NewAPI 计费场景的扩展：
>
> 1. **SQLite 请求日志**（`src/db/`）：toggle errors-only / all 模式，环形缓冲限制 2000 条，记录 metering、context_usage、错误明细。Schema 已对齐 ALLinOne `UsageStore` 便于后续迁移到 Python。
> 2. **每凭据并发限制**（`tokio::Semaphore`）：avoid "把单个号打到死" 的旧调度问题，默认每号 2 并发，可在 admin UI 调整。
> 3. **conversationId 内容指纹**（`src/anthropic/converter.rs::derive_conversation_id_from_messages`）：
>    Anthropic 客户端（Claude Code、curl 等）每次发完整 messages 数组，**没有 sessionId 概念**。upstream 实现 `unwrap_or_else(|| Uuid::new_v4().to_string())` 导致每次都是全新会话，Kiro 后端拿到的永远是 "新 conversation_id + 完整 history"，**无法做 prefix cache，token 全额计费**。
>
>    本 fork 用前 2 条 user 消息内容做 SHA-256 截 16 字节生成稳定 conversationId：同对话连续 turn 命中同一槽，`/compact` 后第一条 user 变了自然换槽。这是所有 Anthropic→AWS 系反代项目的共性问题（不是 kiro.rs 独有）。
>
> 4. **Admin UI 扩展**：请求日志查看页 + 每凭据并发数调整。

---

<table>
<tr>
<td>
<b>特别感谢</b>：<a href="https://co.yes.vg/register?ref=hank9999">YesCode</a> 为本项目提供了 AI API 额度赞助, YesCode 作为一家低调务实的 AI API 中转服务商 <br>
长期以来提供稳定高可用的服务, 如您有意体验, 请点击链接注册体验 → <a href="https://co.yes.vg/register?ref=hank9999">立即访问</a>
</td>
</tr>
</table>

---

#### [LINUX DO 讨论帖](https://linux.do/t/topic/1571986)

## 免责声明

本项目仅供研究使用, Use at your own risk, 使用本项目所导致的任何后果由使用人承担, 与本项目无关。
本项目与 AWS/KIRO/Anthropic/Claude 等官方无关, 本项目不代表官方立场。

## 注意！

因 TLS 默认从 native-tls 切换至 rustls，你可能需要专门安装证书后才能配置 HTTP 代理。可通过 `config.json` 的 `tlsBackend` 切回 `native-tls`。
如果遇到请求报错, 尤其是无法刷新 token, 或者是直接返回 error request, 请尝试切换 tls 后端为 `native-tls`, 一般即可解决。

**Write Failed/会话卡死**: 如果遇到持续的 Write File / Write Failed 并导致会话不可用，参考 Issue [#22](https://github.com/hank9999/kiro.rs/issues/22) 和 [#49](https://github.com/hank9999/kiro.rs/issues/49) 的说明与临时解决方案（通常与输出过长被截断有关，可尝试调低输出相关 token 上限）

## 功能特性

- **Anthropic API 兼容**: 完整支持 Anthropic Claude API 格式
- **流式响应**: 支持 SSE (Server-Sent Events) 流式输出
- **Token 自动刷新**: 自动管理和刷新 OAuth Token
- **多凭据支持**: 支持配置多个凭据，按优先级自动故障转移
- **负载均衡**: 支持 `priority`（按优先级）和 `balanced`（均衡分配）两种模式
- **智能重试**: 单凭据最多重试 3 次，单请求最多重试 9 次
- **凭据回写**: 多凭据格式下自动回写刷新后的 Token
- **Thinking 模式**: 支持 Claude 的 extended thinking 功能
- **工具调用**: 完整支持 function calling / tool use
- **WebSearch**: 内置 WebSearch 工具转换逻辑
- **多模型支持**: 支持 Sonnet、Opus、Haiku 系列模型
- **Admin 管理**: 可选的 Web 管理界面和 API，支持凭据管理、余额查询等
- **多级 Region 配置**: 支持全局和凭据级别的 Auth Region / API Region 配置
- **凭据级代理**: 支持为每个凭据单独配置 HTTP/SOCKS5 代理，优先级：凭据代理 > 全局代理 > 无代理

---

- [开始](#开始)
  - [1. 编译](#1-编译)
  - [2. 最小配置](#2-最小配置)
  - [3. 启动](#3-启动)
  - [4. 验证](#4-验证)
  - [Docker](#docker)
- [配置详解](#配置详解)
  - [config.json](#configjson)
  - [credentials.json](#credentialsjson)
  - [Region 配置](#region-配置)
  - [代理配置](#代理配置)
  - [认证方式](#认证方式)
  - [环境变量](#环境变量)
- [API 端点](#api-端点)
  - [标准端点 (/v1)](#标准端点-v1)
  - [Claude Code 兼容端点 (/cc/v1)](#claude-code-兼容端点-ccv1)
  - [Thinking 模式](#thinking-模式)
  - [工具调用](#工具调用)
- [模型映射](#模型映射)
- [Admin（可选）](#admin可选)
- [注意事项](#注意事项)
- [项目结构](#项目结构)
- [技术栈](#技术栈)
- [License](#license)
- [致谢](#致谢)

## 开始

### 1. 编译

> PS: 如果不想编辑可以直接前往 Release 下载二进制文件

> **前置步骤**：编译前需要先构建前端 Admin UI（用于嵌入到二进制中）：
> ```bash
> cd admin-ui && pnpm install && pnpm build
> ```

```bash
cargo build --release
```

### 2. 最小配置

创建 `config.json`：

```json
{
   "host": "127.0.0.1",
   "port": 8990,
   "apiKey": "sk-kiro-rs-qazWSXedcRFV123456",
   "region": "us-east-1"
}
```
> PS: 如果你需要 Web 管理面板, 请注意配置 `adminApiKey`

创建 `credentials.json`（从 Kiro IDE 等中获取凭证信息）：
> PS: 可以前往 Web 管理面板配置跳过本步骤
> 如果你对凭据地域有疑惑, 请查看 [Region 配置](#region-配置)

Social 认证：
```json
{
   "refreshToken": "你的刷新token",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "social"
}
```

IdC 认证：
```json
{
   "refreshToken": "你的刷新token",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "idc",
   "clientId": "你的clientId",
   "clientSecret": "你的clientSecret"
}
```

### 3. 启动

```bash
./target/release/kiro-rs
```

或指定配置文件路径：

```bash
./target/release/kiro-rs -c /path/to/config.json --credentials /path/to/credentials.json
```

### 4. 验证

```bash
curl http://127.0.0.1:8990/v1/messages \
  -H "Content-Type: application/json" \
  -H "x-api-key: sk-kiro-rs-qazWSXedcRFV123456" \
  -d '{
    "model": "claude-sonnet-4-20250514",
    "max_tokens": 1024,
    "stream": true,
    "messages": [
      {"role": "user", "content": "Hello, Claude!"}
    ]
  }'
```

### Docker

也可以通过 Docker 启动：

```bash
docker-compose up
```

需要将 `config.json` 和 `credentials.json` 挂载到容器中，具体参见 `docker-compose.yml`。

## 配置详解

### config.json

| 字段 | 类型 | 默认值 | 描述 |
|------|------|--------|------|
| `host` | string | `127.0.0.1` | 服务监听地址 |
| `port` | number | `8080` | 服务监听端口 |
| `apiKey` | string | - | 自定义 API Key（用于客户端认证，必配） |
| `region` | string | `us-east-1` | AWS 区域 |
| `authRegion` | string | - | Auth Region（用于 Token 刷新），未配置时回退到 region |
| `apiRegion` | string | - | API Region（用于 API 请求），未配置时回退到 region |
| `kiroVersion` | string | `0.9.2` | Kiro 版本号 |
| `machineId` | string | - | 自定义机器码（64位十六进制），不定义则自动生成 |
| `systemVersion` | string | 随机 | 系统版本标识 |
| `nodeVersion` | string | `22.21.1` | Node.js 版本标识 |
| `tlsBackend` | string | `rustls` | TLS 后端：`rustls` 或 `native-tls` |
| `countTokensApiUrl` | string | - | 外部 count_tokens API 地址 |
| `countTokensApiKey` | string | - | 外部 count_tokens API 密钥 |
| `countTokensAuthType` | string | `x-api-key` | 外部 API 认证类型：`x-api-key` 或 `bearer` |
| `proxyUrl` | string | - | HTTP/SOCKS5 代理地址 |
| `proxyUsername` | string | - | 代理用户名 |
| `proxyPassword` | string | - | 代理密码 |
| `adminApiKey` | string | - | Admin API 密钥，配置后启用凭据管理 API 和 Web 管理界面 |
| `loadBalancingMode` | string | `priority` | 负载均衡模式：`priority`（按优先级）或 `balanced`（均衡分配） |
| `extractThinking` | boolean | `true` | 非流式响应的 thinking 块提取。启用后 `<thinking>` 标签会被解析为独立的 `thinking` 内容块 |
| `defaultEndpoint` | string | `ide` | 默认 Kiro 端点。凭据未显式指定 `endpoint` 时使用。当前支持：`ide` |

完整配置示例：

```json
{
   "host": "127.0.0.1",
   "port": 8990,
   "apiKey": "sk-kiro-rs-qazWSXedcRFV123456",
   "region": "us-east-1",
   "tlsBackend": "rustls",
   "kiroVersion": "0.9.2",
   "machineId": "64位十六进制机器码",
   "systemVersion": "darwin#24.6.0",
   "nodeVersion": "22.21.1",
   "authRegion": "us-east-1",
   "apiRegion": "us-east-1",
   "countTokensApiUrl": "https://api.example.com/v1/messages/count_tokens",
   "countTokensApiKey": "sk-your-count-tokens-api-key",
   "countTokensAuthType": "x-api-key",
   "proxyUrl": "http://127.0.0.1:7890",
   "proxyUsername": "user",
   "proxyPassword": "pass",
   "adminApiKey": "sk-admin-your-secret-key",
   "loadBalancingMode": "priority",
   "extractThinking": true
}
```

### credentials.json

支持单对象格式（向后兼容）或数组格式（多凭据）。

#### 字段说明

| 字段             | 类型     | 描述                                          |
|----------------|--------|---------------------------------------------|
| `id`           | number | 凭据唯一 ID（可选，仅用于 Admin API 管理；手写文件可不填）        |
| `accessToken`  | string | OAuth 访问令牌（可选，可自动刷新）                        |
| `refreshToken` | string | OAuth 刷新令牌                                  |
| `profileArn`   | string | AWS Profile ARN（可选，登录时返回）                   |
| `expiresAt`    | string | Token 过期时间 (RFC3339)                        |
| `authMethod`   | string | 认证方式：`social` 或 `idc`                       |
| `clientId`     | string | IdC 登录的客户端 ID（IdC 认证必填）                     |
| `clientSecret` | string | IdC 登录的客户端密钥（IdC 认证必填）                      |
| `priority`     | number | 凭据优先级，数字越小越优先，默认为 0                         |
| `region`       | string | 凭据级 Auth Region, 兼容字段                       |
| `authRegion`   | string | 凭据级 Auth Region，用于 Token 刷新, 未配置时回退到 region |
| `apiRegion`    | string | 凭据级 API Region，用于 API 请求                    |
| `machineId`    | string | 凭据级机器码（64位十六进制）                             |
| `email`        | string | 用户邮箱（可选，从 API 获取）                           |
| `proxyUrl`     | string | 凭据级代理 URL（可选，特殊值 `direct` 表示不使用代理）       |
| `proxyUsername`| string | 凭据级代理用户名（可选）                                |
| `proxyPassword`| string | 凭据级代理密码（可选）                                 |
| `endpoint`     | string | 凭据级端点名称（可选，未配置时使用 `config.defaultEndpoint`）|

说明：
- IdC / Builder-ID / IAM 在本项目里属于同一种登录方式，配置时统一使用 `authMethod: "idc"`
- 为兼容旧配置，`builder-id` / `iam` 仍可被识别，但会按 `idc` 处理

#### 单凭据格式（旧格式，向后兼容）

```json
{
   "accessToken": "请求token，一般有效期一小时，可选",
   "refreshToken": "刷新token，一般有效期7-30天不等",
   "profileArn": "arn:aws:codewhisperer:us-east-1:111112222233:profile/QWER1QAZSDFGH",
   "expiresAt": "2025-12-31T02:32:45.144Z",
   "authMethod": "social",
   "clientId": "IdC 登录需要",
   "clientSecret": "IdC 登录需要"
}
```

#### 多凭据格式（支持故障转移和自动回写）

```json
[
   {
      "refreshToken": "第一个凭据的刷新token",
      "expiresAt": "2025-12-31T02:32:45.144Z",
      "authMethod": "social",
      "priority": 0
   },
   {
      "refreshToken": "第二个凭据的刷新token",
      "expiresAt": "2025-12-31T02:32:45.144Z",
      "authMethod": "idc",
      "clientId": "xxxxxxxxx",
      "clientSecret": "xxxxxxxxx",
      "region": "us-east-2",
      "priority": 1,
      "proxyUrl": "socks5://proxy.example.com:1080",
      "proxyUsername": "user",
      "proxyPassword": "pass"
   },
   {
      "refreshToken": "第三个凭据（显式不走代理）",
      "expiresAt": "2025-12-31T02:32:45.144Z",
      "authMethod": "social",
      "priority": 2,
      "proxyUrl": "direct"
   }
]
```

多凭据特性：
- 按 `priority` 字段排序，数字越小优先级越高（默认为 0）
- 单凭据最多重试 3 次，单请求最多重试 9 次
- 自动故障转移到下一个可用凭据
- 多凭据格式下 Token 刷新后自动回写到源文件

### Region 配置

支持多级 Region 配置，分别控制 Token 刷新和 API 请求使用的区域。

**Auth Region**（Token 刷新）优先级：
`凭据.authRegion` > `凭据.region` > `config.authRegion` > `config.region`

**API Region**（API 请求）优先级：
`凭据.apiRegion` > `config.apiRegion` > `config.region`

### 代理配置

支持全局代理和凭据级代理，凭据级代理会覆盖该凭据产生的所有出站连接（API 请求、Token 刷新、额度查询）。

**代理优先级**：`凭据.proxyUrl` > `config.proxyUrl` > 无代理

| 凭据 `proxyUrl` 值 | 行为 |
|---|---|
| 具体 URL（如 `http://proxy:8080`、`socks5://proxy:1080`） | 使用凭据指定的代理 |
| `direct` | 显式不使用代理（即使全局配置了代理） |
| 未配置（留空） | 回退到全局代理配置 |

凭据级代理示例：

```json
[
   {
      "refreshToken": "凭据A：使用自己的代理",
      "authMethod": "social",
      "proxyUrl": "socks5://proxy-a.example.com:1080",
      "proxyUsername": "user_a",
      "proxyPassword": "pass_a"
   },
   {
      "refreshToken": "凭据B：显式不走代理（直连）",
      "authMethod": "social",
      "proxyUrl": "direct"
   },
   {
      "refreshToken": "凭据C：使用全局代理（或直连，取决于 config.json）",
      "authMethod": "social"
   }
]
```

### 认证方式

客户端请求本服务时，支持两种认证方式：

1. **x-api-key Header**
   ```
   x-api-key: sk-your-api-key
   ```

2. **Authorization Bearer**
   ```
   Authorization: Bearer sk-your-api-key
   ```

### 环境变量

可通过环境变量配置日志级别：

```bash
RUST_LOG=debug ./target/release/kiro-rs
```

#### 实验性开关（默认全部关闭，生产勿开）

| 变量 | 默认 | 说明 |
|------|------|------|
| `KIRO_TOOLS_IN_PREFIX` | off | 把 tools 挪到 `history[0]` 前缀尝试进缓存区。**实测 Kiro 返回 400，已证伪** |
| `KIRO_CACHE_POINT` | off | 把 Anthropic `cache_control` 翻译为 Kiro `cachePoint`。**实测 no-op，见下文「计费与缓存机制」** |
| `KIRO_CACHE_POINT_TYPE` | `EPHEMERAL` | cachePoint 的 type；仅 `default` 被 Kiro 接受，其余值报 400 |
| `KIRO_CACHE_POINT_CONFIG` | `1` | 是否附带 `clientCacheConfig` |
| `KIRO_CACHE_POINT_PLACE` | `current` | cachePoint 放 `current`(当前消息) 还是 `history`(历史末尾 user) |

## 计费与缓存机制（重要 —— Python 迁移必读）

本分支的核心目标：让 NewAPI 等中转网关能按 Kiro 的真实 metering / 缓存正确计费。
以下是 2026-05 期间逐项实测得出的关键结论。

### 1. Kiro 按 `conversationId` 自动做 prefix-cache

Anthropic API 客户端（Claude Code 等）每轮都发完整 messages，没有 sessionId。
Kiro/CodeWhisperer 后端用 `conversationId` 跟踪会话做 prefix cache。

- **务必让 `conversationId` 稳定**：本项目用「前 2 条 user 消息内容的 SHA-256」派生（见 `converter.rs::derive_conversation_id_from_messages`）。`agentContinuationId` 也必须稳定（由 conversationId 派生），否则实测 metering ~3 倍。
- 一旦 conversationId 稳定，**Kiro 自动缓存生效**：实测同一会话冷启动 `cached=0`，第 2 轮起命中 ~68%（长对话可达 ~92%），metering 同步下降。
- **这是降低输入成本的唯一有效杠杆。**

### 2. `tokenUsageEvent` 给出精确 token（用于计费）

Kiro 流式响应里有 `tokenUsageEvent`，字段：
`uncachedInputTokens` / `outputTokens` / `totalTokens` / `cacheReadInputTokens` / `cacheWriteInputTokens`。

- `prompt_tokens = uncachedInputTokens + cacheReadInputTokens`
- `cached_tokens  = cacheReadInputTokens`
- `completion_tokens = outputTokens`（已含 thinking）

解析见 `kiro/model/events/token_usage.rs` + `anthropic/logging.rs`。有精确值时跳过启发式估算。

### 3. `cachePoint` 翻译是 no-op（已证伪，勿在 Python 重做）

尝试把 Anthropic `cache_control` 翻译成 Kiro `userInputMessage.cachePoint`，结论：

- `type` 必须为 `"default"`；`EPHEMERAL`/`PERSISTENT` → 400 "Improperly formed request."。
- 即使用 `default`，A/B 实测缓存命中、metering **与不发 cachePoint 完全一致** —— Kiro 接受该字段但忽略它，缓存完全由 `conversationId` 自动驱动。
- 因此 `cache_control → cachePoint` **无收益**，代码仅作 dormant 实验保留（`KIRO_CACHE_POINT`）。

> 一句话总结给 Python 迁移：**只需稳定 conversationId（拿自动缓存）+ 解析 tokenUsageEvent（拿精确计费）。不要碰 cachePoint，不要把 tools 挪进 history。**

### 4. 命中率上限 ~52%：三方对照调查（2026-05-24）

实测大会话命中率稳定在 ~52%（小会话更高、大会话更低，`max_cached` 约 40 万 token 封顶）。
为确认"是不是漏了字段"，对照了两个同类项目 + 社区：

| 维度 | 本项目 kiro.rs | AIClient-2-API | jwadow/kiro-gateway（成熟） |
|------|------|------|------|
| `conversationId` | 前 2 条 user 哈希，**同会话稳定** | 每次新 UUID | 默认随机 UUID（调用未传 messages）；传了也含"最后一条"每轮变 |
| `agentContinuationId` | ✅ 有 | ❌ 无 | ❌ 无 |
| `cacheReadInputTokens` | tokenUsageEvent 精确 + estimate 回退 | 硬编码 `0` | 读，但注释 *only forwarded when explicitly returned by upstream* |
| `cache_control`/cachePoint | 实测证伪，dormant | 不发 | 注释 *not supported by Kiro*，直接丢弃 |

**结论**：没有漏任何影响 prefix cache 的字段；本项目的 conversationId 稳定性 + agentContinuationId 是三者中最完整的。
两个独立信源（kiro-gateway 源码注释）印证本项目结论：①`cache_control` 不被 Kiro 支持 → cachePoint 无效；
②`cacheRead` 仅在 Kiro 显式返回时才有 → 解释 opus-4-6 缺该字段。社区（linux.do）多数实现是随机 UUID、
**命中率≈0**（"hello 一下耗 0.5 刀"），本项目的 52% 已是生态最优；kiro-gateway 的 issues 里**无人讨论过命中率优化**。

⇒ **52% 是 Kiro 服务端缓存上限，客户端无法突破。** 降本只能缩短会话（控制 prompt < ~40 万 token）。

**唯一尚未验证的路径（TODO）**：抓 **kiro 官方 CLI** 的真实出站包，对比官方客户端是否发了任何我们/上述项目都没发的字段
（如 workspaceState / supplementaryContext / 某 profile 维度），以及官方在长会话下的真实命中率是否也只有 ~52%。
若官方也是 ~52%，则坐实为服务端上限；若官方更高，则抓包 diff 出差异字段即是突破口。

## API 端点

### 标准端点 (/v1)

| 端点 | 方法 | 描述 |
|------|------|------|
| `/v1/models` | GET | 获取可用模型列表 |
| `/v1/messages` | POST | 创建消息（对话） |
| `/v1/messages/count_tokens` | POST | 估算 Token 数量 |

### Claude Code 兼容端点 (/cc/v1)

| 端点 | 方法 | 描述 |
|------|------|------|
| `/cc/v1/messages` | POST | 创建消息（缓冲模式，确保 `input_tokens` 准确） |
| `/cc/v1/messages/count_tokens` | POST | 估算 Token 数量（与 `/v1` 相同） |

> **`/cc/v1/messages` 与 `/v1/messages` 的区别**：
> - `/v1/messages`：实时流式返回，`message_start` 中的 `input_tokens` 是估算值
> - `/cc/v1/messages`：缓冲模式，等待上游流完成后，用从 `contextUsageEvent` 计算的准确 `input_tokens` 更正 `message_start`，然后一次性返回所有事件
> - 等待期间会每 25 秒发送 `ping` 事件保活

### Thinking 模式

支持 Claude 的 extended thinking 功能：

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 16000,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 10000
  },
  "messages": [...]
}
```

### 工具调用

完整支持 Anthropic 的 tool use 功能：

```json
{
  "model": "claude-sonnet-4-20250514",
  "max_tokens": 1024,
  "tools": [
    {
      "name": "get_weather",
      "description": "获取指定城市的天气",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": {"type": "string"}
        },
        "required": ["city"]
      }
    }
  ],
  "messages": [...]
}
```

## 模型映射

| Anthropic 模型 | Kiro 模型 |
|----------------|-----------|
| `*sonnet*` | `claude-sonnet-4.5` |
| `*opus*`（含 4.5/4-5） | `claude-opus-4.5` |
| `*opus*`（其他） | `claude-opus-4.6` |
| `*haiku*` | `claude-haiku-4.5` |

## Admin（可选）

当 `config.json` 配置了非空 `adminApiKey` 时，会启用：

- **Admin API（认证同 API Key）**
  - `GET /api/admin/credentials` - 获取所有凭据状态
  - `POST /api/admin/credentials` - 添加新凭据
  - `DELETE /api/admin/credentials/:id` - 删除凭据
  - `POST /api/admin/credentials/:id/disabled` - 设置凭据禁用状态
  - `POST /api/admin/credentials/:id/priority` - 设置凭据优先级
  - `POST /api/admin/credentials/:id/reset` - 重置失败计数
  - `GET /api/admin/credentials/:id/balance` - 获取凭据余额

- **Admin UI**
  - `GET /admin` - 访问管理页面（需要在编译前构建 `admin-ui/dist`）

## 注意事项

1. **凭证安全**: 请妥善保管 `credentials.json` 文件，不要提交到版本控制
2. **Token 刷新**: 服务会自动刷新过期的 Token，无需手动干预
3. **WebSearch 工具**: 当 `tools` 列表仅包含一个 `web_search` 工具时，会走内置 WebSearch 转换逻辑

## 项目结构

```
kiro-rs/
├── src/
│   ├── main.rs                 # 程序入口
│   ├── http_client.rs          # HTTP 客户端构建
│   ├── token.rs                # Token 计算模块
│   ├── debug.rs                # 调试工具
│   ├── test.rs                 # 测试
│   ├── model/                  # 配置和参数模型
│   │   ├── config.rs           # 应用配置
│   │   └── arg.rs              # 命令行参数
│   ├── anthropic/              # Anthropic API 兼容层
│   │   ├── router.rs           # 路由配置
│   │   ├── handlers.rs         # 请求处理器
│   │   ├── middleware.rs       # 认证中间件
│   │   ├── types.rs            # 类型定义
│   │   ├── converter.rs        # 协议转换器
│   │   ├── stream.rs           # 流式响应处理
│   │   └── websearch.rs        # WebSearch 工具处理
│   ├── kiro/                   # Kiro API 客户端
│   │   ├── provider.rs         # API 提供者
│   │   ├── token_manager.rs    # Token 管理
│   │   ├── machine_id.rs       # 设备指纹生成
│   │   ├── model/              # 数据模型
│   │   │   ├── credentials.rs  # OAuth 凭证
│   │   │   ├── events/         # 响应事件类型
│   │   │   ├── requests/       # 请求类型
│   │   │   ├── common/         # 共享类型
│   │   │   ├── token_refresh.rs # Token 刷新模型
│   │   │   └── usage_limits.rs # 使用额度模型
│   │   └── parser/             # AWS Event Stream 解析器
│   │       ├── decoder.rs      # 流式解码器
│   │       ├── frame.rs        # 帧解析
│   │       ├── header.rs       # 头部解析
│   │       ├── error.rs        # 错误类型
│   │       └── crc.rs          # CRC 校验
│   ├── admin/                  # Admin API 模块
│   │   ├── router.rs           # 路由配置
│   │   ├── handlers.rs         # 请求处理器
│   │   ├── service.rs          # 业务逻辑服务
│   │   ├── types.rs            # 类型定义
│   │   ├── middleware.rs       # 认证中间件
│   │   └── error.rs            # 错误处理
│   ├── admin_ui/               # Admin UI 静态文件嵌入
│   │   └── router.rs           # 静态文件路由
│   └── common/                 # 公共模块
│       └── auth.rs             # 认证工具函数
├── admin-ui/                   # Admin UI 前端工程（构建产物会嵌入二进制）
├── tools/                      # 辅助工具
├── Cargo.toml                  # 项目配置
├── config.example.json         # 配置示例
├── docker-compose.yml          # Docker Compose 配置
└── Dockerfile                  # Docker 构建文件
```

## 技术栈

- **Web 框架**: [Axum](https://github.com/tokio-rs/axum) 0.8
- **异步运行时**: [Tokio](https://tokio.rs/)
- **HTTP 客户端**: [Reqwest](https://github.com/seanmonstar/reqwest)
- **序列化**: [Serde](https://serde.rs/)
- **日志**: [tracing](https://github.com/tokio-rs/tracing)
- **命令行**: [Clap](https://github.com/clap-rs/clap)

## License

MIT

## 致谢

本项目的实现离不开前辈的努力:  
 - [kiro2api](https://github.com/caidaoli/kiro2api)
 - [proxycast](https://github.com/aiclientproxy/proxycast)

本项目部分逻辑参考了以上的项目, 再次由衷的感谢!
