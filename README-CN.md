# OpenProxy

OpenProxy 是一个使用 Rust/Axum 编写的 API 格式转换代理，核心目标是让只支持 OpenAI Chat Completions 格式的上游服务，也能被兼容 OpenAI Responses API 的客户端调用。

项目提供 `/v1/responses` 到上游 `/chat/completions` 的完整转换，同时保留 `/v1/chat/completions` 直通代理，并支持 `/v1/models` 模型列表代理。

## 热点功能速览

| 功能 | 实现内容 | 价值 |
| --- | --- | --- |
| Responses API 桥接 | 将 `/v1/responses` 转换为上游 Chat Completions | 让 Responses API 客户端直接使用只支持 Chat Completions 的上游 |
| SSE 流式转换 | 将 Chat Completions SSE 转为 Responses API SSE 事件 | 保持流式输出、工具调用增量和完成事件兼容 |
| 多上游模型路由 | 聚合 `/v1/models`，并根据请求 `model` 自动选择上游 | 一个 OpenAI 兼容入口管理多个模型提供方 |
| 工具调用适配 | 转换 tools、tool_choice 和流式工具调用参数 | 支持 agent/tool 工作流跨 API 格式运行 |
| 重试与错误透传 | 对 403/429/5xx 自动重试，并保留上游错误 body | 提升稳定性，同时方便观察真实上游错误 |
| OpenAI 兼容认证 | 默认发送 `Authorization: Bearer <api_key>` | 开箱兼容 OpenAI 风格上游，也支持自定义认证 header |

## 功能特性

- **Responses API 转 Chat Completions**
  - 将 `/v1/responses` 请求转换为上游 Chat Completions 请求。
  - 支持 `instructions` 转换为 system message。
  - 支持字符串输入与完整 `input[]` 对话上下文。
  - 保留 user、assistant、tool 等消息顺序。
  - 支持 `max_output_tokens`、`temperature`、`top_p` 等常用参数转换。

- **Chat Completions 转 Responses API**
  - 将上游非流式 Chat Completions 响应转换为 Responses API 响应。
  - 支持普通文本输出转换为 `message` output item。
  - 支持 `tool_calls` 转换为 Responses API 的 `function_call` output item。
  - 生成 Responses API 兼容的 response id、output、usage 和状态字段。

- **完整 SSE 流式转换**
  - 支持上游 Chat Completions SSE 流转换为 Responses API SSE 事件。
  - 支持 `response.created`、`response.in_progress`、`response.output_item.added`、`response.output_text.delta`、`response.completed` 等事件。
  - 内置 SSE 解码器，能正确处理 TCP 分片、半包、跨 chunk event。
  - 支持中文、emoji 等多字节字符跨 chunk 分片时不损坏内容。
  - 当上游直接发送 `[DONE]` 或连接关闭时，会兜底完成未结束的输出项。

- **工具调用适配**
  - 支持 Responses API `tools` 转 Chat Completions `tools`。
  - 支持 Responses API function `tool_choice` 转 Chat Completions 兼容格式。
  - 支持流式工具调用参数增量：`response.function_call_arguments.delta`。
  - 支持工具调用完成事件：`response.function_call_arguments.done` 与 `response.output_item.done`。

- **错误重试与错误透传**
  - 对上游 `403`、`429`、`5xx` 自动重试。
  - 请求发送错误也会自动重试。
  - 默认最多重试 3 次。
  - 日志会明确输出每次尝试：`attempt 1/3`、`attempt 2/3`、`attempt 3/3`。
  - 上游返回错误时，会透传状态码和响应 body，并在日志中输出上游错误内容。

- **模型列表代理**
  - 支持 `GET /v1/models`。
  - 单上游模式下自动代理到上游 `{upstream.url}/models`。
  - 多上游模式下自动聚合所有上游的模型列表。
  - 同样支持认证、重试和错误 body 透传。

- **多上游模型路由**
  - 同时兼容旧版 `[upstream]` 和新版 `[[upstreams]]` 配置。
  - `/v1/models` 会聚合所有上游模型列表。
  - 支持给上游设置 `name`，模型列表显示为 `name:model`。
  - `/v1/responses` 和 `/v1/chat/completions` 会根据请求里的 `model` 自动匹配上游。
  - 请求使用 `name:model` 时会路由到该上游，并在转发前还原为上游原始模型名。
  - 如果多个上游暴露同名模型，按配置顺序选择第一个匹配项。

- **认证配置兼容**
  - `auth_header` 省略或为空时，默认使用 OpenAI 标准 `Authorization`。
  - 当 header 为 `Authorization` 且 `api_key` 未包含 `Bearer ` 前缀时，会自动发送 `Authorization: Bearer <api_key>`。
  - 如果 `api_key` 已经是 `Bearer sk-xxx`，不会重复添加前缀。
  - 自定义 header（如 `api-key`）时，会原样发送 `api_key`。

## 技术栈

- **语言**：Rust 2024 Edition
- **异步运行时**：Tokio
- **Web 框架**：Axum
- **上游 HTTP 客户端**：Reqwest
- **序列化**：Serde / serde_json
- **流式处理**：SSE / futures
- **配置文件**：TOML
- **日志**：tracing / tracing-subscriber
- **ID 生成**：UUID v4

## 快速开始

### 1. 安装

通过 npm 安装：

```bash
npm install -g @grenanhao/openproxy
```

运行已安装的 CLI：

```bash
openproxy ./config.toml
```

也可以从 GitHub Releases 下载原生二进制：

```text
https://github.com/GrenAnHao/openai-responses-proxy/releases
```

或者从源码编译：

```bash
cargo build
```

### 2. 配置

默认读取项目根目录下的 `config.toml`。先从示例配置复制：

```bash
cp config.example.toml config.toml
```

OpenAI 风格上游示例：

```toml
port = 8080

[upstream]
url = "https://api.openai.com/v1"
api_key = "sk-your-api-key"
```

上述配置会自动发送：

```http
Authorization: Bearer sk-your-api-key
```

自定义认证 header 示例：

```toml
port = 8080

[upstream]
url = "http://127.0.0.1:8000/v1"
api_key = "your-secret"
auth_header = "api-key"
```

上述配置会发送：

```http
api-key: your-secret
```

多上游自动路由示例：

```toml
port = 8080

[[upstreams]]
name = "openai"
url = "https://api.openai.com/v1"
api_key = "sk-your-api-key"

[[upstreams]]
name = "local"
url = "http://127.0.0.1:8000/v1"
api_key = "your-secret"
auth_header = "api-key"
```

使用上述配置时，`GET /v1/models` 会返回合并后的模型列表，并显示类似 `openai:gpt-4.1-mini` 或 `local:qwen3` 的模型 id。请求 `/v1/responses` 或 `/v1/chat/completions` 时可以直接使用这些显示出来的 id；OpenProxy 会路由到对应上游，并在转发前去掉 `name:` 前缀。

### 3. 从源码运行

```bash
cargo run
```

也可以指定配置文件路径：

```bash
cargo run -- ./config.toml
```

启动后默认监听：

```text
0.0.0.0:8080
```

## 部署命令

通过 npm 安装并运行：

```bash
npm install -g @grenanhao/openproxy
openproxy ./config.toml
```

使用下载的 Release 二进制运行：

```bash
./openproxy ./config.toml
```

发布 GitHub Release：

```bash
git tag -a v0.1.0 -m "OpenProxy v0.1.0"
git push origin v0.1.0
```

通过 GitHub Actions 发布 npm 包：

```bash
gh secret set NPM_TOKEN
gh workflow run "Publish NPM" --ref master
```

`NPM_TOKEN` 必须是有权限发布 `@grenanhao/openproxy` 的 npm automation token。由于 GitHub Actions 创建 Release 时不会稳定触发另一个 release workflow，建议在 Release 成功后手动执行上面的 `gh workflow run` 命令发布 npm 包。

## 接口说明

### `POST /v1/responses`

接收 Responses API 请求，转换为上游 Chat Completions 请求。

非流式请求示例：

```bash
curl http://127.0.0.1:8080/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "input": "你好，介绍一下你自己"
  }'
```

流式请求示例：

```bash
curl http://127.0.0.1:8080/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "stream": true,
    "input": "请流式输出一段中文"
  }'
```

完整上下文示例：

```json
{
  "model": "gpt-4.1-mini",
  "instructions": "你是一个简洁的助手。",
  "input": [
    {
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "你好" }]
    },
    {
      "type": "message",
      "role": "assistant",
      "content": [{ "type": "output_text", "text": "你好，有什么可以帮你？" }]
    },
    {
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "继续上一个话题" }]
    }
  ]
}
```

### `POST /v1/chat/completions`

直通代理到上游 `{upstream.url}/chat/completions`。

适合已经使用 Chat Completions 格式的客户端。多上游模式下会读取请求中的 `model` 并路由到匹配上游。

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "messages": [
      { "role": "user", "content": "你好" }
    ]
  }'
```

### `GET /v1/models`

代理上游模型列表。多上游模式下会聚合所有已配置上游的模型列表。

```bash
curl http://127.0.0.1:8080/v1/models
```

## 配置项

| 字段 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `port` | 否 | `8080` | 本地监听端口 |
| `upstream.url` | 旧版单上游配置必填 | 无 | 上游 API 基础地址，通常以 `/v1` 结尾 |
| `upstream.api_key` | 否 | 空字符串 | 上游认证密钥 |
| `upstream.auth_header` | 否 | `Authorization` | 上游认证 header 名；为空时也使用 `Authorization` |
| `upstreams[].name` | 否 | 无 | 模型显示和路由前缀，会生成类似 `name:model` 的模型 id |
| `upstreams[].url` | 多上游配置必填 | 无 | 单个上游的 API 基础地址 |
| `upstreams[].api_key` | 否 | 空字符串 | 单个上游的认证密钥 |
| `upstreams[].auth_header` | 否 | `Authorization` | 单个上游的认证 header 名 |

## 转换流程

`/v1/responses` 的核心流程：

1. 解析客户端 Responses API 请求。
2. 根据请求里的 `model` 查找暴露该模型的上游。
3. 将 `instructions`、`input`、`tools`、`tool_choice` 等字段转换为 Chat Completions 兼容格式。
4. 请求上游 `{upstream.url}/chat/completions`。
5. 非流式响应：解析 Chat Completions JSON，转换为 Responses API JSON。
6. 流式响应：解析上游 SSE，逐事件转换为 Responses API SSE。
7. 上游错误：按规则重试，最终仍失败时透传状态码和错误 body。

## 开发命令

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

常用命令：

```bash
cargo fmt
cargo run
cargo test <test_name>
```

## 当前测试覆盖

项目测试覆盖了以下关键场景：

- Responses 输入上下文完整转换。
- Responses tool/tool_choice 转 Chat Completions。
- 非流式 tool_calls 转 Responses function_call。
- 流式文本增量转换。
- 流式工具调用参数 delta/done。
- SSE 半包与多字节字符分片。
- `[DONE]` 无 finish_reason 时兜底完成 output。
- 403/429/5xx 重试。
- `/v1/models` 代理与重试。
- 多上游 `/v1/models` 聚合。
- 根据请求模型自动路由到对应上游。
- OpenAI 默认 `Authorization: Bearer <api_key>` 认证行为。

## 注意事项

- 不建议将真实 `api_key` 提交到版本控制。
- 如果使用 OpenAI 标准认证，可以省略 `auth_header`。
- 如果上游使用非标准认证 header，请显式配置 `auth_header`。
- 只有 `403`、`429`、`5xx` 和请求发送错误会自动重试；`401` 通常代表认证失败，不会重试。
