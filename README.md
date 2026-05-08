# OpenProxy

[中文文档](./README-CN.md)

OpenProxy is a Rust/Axum API conversion proxy. It lets clients that speak the OpenAI Responses API call upstream providers that only support the OpenAI Chat Completions format.

The project converts `/v1/responses` requests to upstream `/chat/completions` requests, keeps a direct `/v1/chat/completions` passthrough route, and also proxies `/v1/models`.

## Hot Features

| Feature | What it does | Why it matters |
| --- | --- | --- |
| Responses API bridge | Converts `/v1/responses` to upstream Chat Completions | Use Responses-compatible clients with Chat Completions-only providers |
| Streaming SSE conversion | Converts Chat Completions SSE into Responses API SSE events | Keeps streaming output, tool call deltas, and final completion events compatible |
| Multi-upstream model routing | Aggregates `/v1/models` and routes requests by `model` | Use multiple providers behind one OpenAI-compatible endpoint |
| Tool call adaptation | Converts tools, tool_choice, and streamed tool call arguments | Enables agent/tool workflows across different API formats |
| Retry and error passthrough | Retries 403/429/5xx and preserves upstream error bodies | Improves reliability while keeping upstream failures observable |
| OpenAI-compatible auth | Defaults to `Authorization: Bearer <api_key>` | Works out of the box with OpenAI-style providers and custom auth headers |

## Features

- **Responses API to Chat Completions**
  - Converts incoming `/v1/responses` requests into upstream Chat Completions requests.
  - Converts `instructions` into a system message.
  - Supports string input and full `input[]` conversation context.
  - Preserves user, assistant, and tool message order.
  - Converts common parameters such as `max_output_tokens`, `temperature`, and `top_p`.

- **Chat Completions to Responses API**
  - Converts non-streaming upstream Chat Completions responses into Responses API responses.
  - Converts plain text output into `message` output items.
  - Converts `tool_calls` into Responses API `function_call` output items.
  - Generates Responses-compatible response ids, output items, usage, and status fields.

- **Full SSE streaming conversion**
  - Converts upstream Chat Completions SSE streams into Responses API SSE events.
  - Supports events such as `response.created`, `response.in_progress`, `response.output_item.added`, `response.output_text.delta`, and `response.completed`.
  - Includes an SSE decoder that handles TCP chunking, partial packets, and events split across chunks.
  - Preserves multibyte characters such as Chinese text and emoji when split across chunks.
  - Flushes unfinished output items when upstream sends `[DONE]` or closes the connection.

- **Tool call adaptation**
  - Converts Responses API `tools` into Chat Completions `tools`.
  - Converts Responses API function `tool_choice` into a Chat Completions-compatible shape.
  - Supports streaming tool call argument deltas via `response.function_call_arguments.delta`.
  - Emits tool call completion events via `response.function_call_arguments.done` and `response.output_item.done`.

- **Retry and error passthrough**
  - Automatically retries upstream `403`, `429`, and `5xx` responses.
  - Retries request send failures as well.
  - Uses up to 3 attempts by default.
  - Logs each attempt clearly: `attempt 1/3`, `attempt 2/3`, `attempt 3/3`.
  - Passes through upstream error status and body, and logs upstream error bodies.

- **Model list proxy**
  - Supports `GET /v1/models`.
  - Proxies to upstream `{upstream.url}/models` for single-upstream setups.
  - Merges model lists from all configured upstreams for multi-upstream setups.
  - Uses the same authentication, retry, and error body passthrough behavior.

- **Multi-upstream model routing**
  - Supports both legacy `[upstream]` and new `[[upstreams]]` configuration.
  - Aggregates `/v1/models` across all upstream providers.
  - Supports optional upstream names and displays models as `name:model`.
  - Automatically selects the upstream that lists the requested `model`.
  - Requests using `name:model` are routed to that upstream and forwarded as the raw upstream model id.
  - If the same model exists in multiple upstreams, the first matching upstream in config order is used.

- **Authentication compatibility**
  - If `auth_header` is omitted or empty, OpenAI's standard `Authorization` header is used.
  - When the header is `Authorization` and `api_key` does not already start with `Bearer `, OpenProxy sends `Authorization: Bearer <api_key>`.
  - If `api_key` already starts with `Bearer `, the prefix is not duplicated.
  - Custom headers such as `api-key` send the raw `api_key` value.

## Tech Stack

- **Language**: Rust 2024 Edition
- **Async runtime**: Tokio
- **Web framework**: Axum
- **Upstream HTTP client**: Reqwest
- **Serialization**: Serde / serde_json
- **Streaming**: SSE / futures
- **Configuration**: TOML
- **Logging**: tracing / tracing-subscriber
- **ID generation**: UUID v4

## Quick Start

### 1. Build

```bash
cargo build
```

### 2. Configure

OpenProxy reads `config.toml` from the project root by default. Start from the example file:

```bash
cp config.example.toml config.toml
```

OpenAI-style upstream:

```toml
port = 8080

[upstream]
url = "https://api.openai.com/v1"
api_key = "sk-your-api-key"
```

This automatically sends:

```http
Authorization: Bearer sk-your-api-key
```

Custom authentication header:

```toml
port = 8080

[upstream]
url = "http://127.0.0.1:8000/v1"
api_key = "your-secret"
auth_header = "api-key"
```

This sends:

```http
api-key: your-secret
```

Multi-upstream routing:

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

With this configuration, `GET /v1/models` returns a merged model list and displays ids such as `openai:gpt-4.1-mini` or `local:qwen3`. Requests to `/v1/responses` and `/v1/chat/completions` can use those displayed ids; OpenProxy routes to the named upstream and strips the `name:` prefix before forwarding.

### 3. Run

```bash
cargo run
```

You can also pass a custom config path:

```bash
cargo run -- ./config.toml
```

By default, OpenProxy listens on:

```text
0.0.0.0:8080
```

## API Endpoints

### `POST /v1/responses`

Accepts a Responses API request and converts it into an upstream Chat Completions request.

Non-streaming example:

```bash
curl http://127.0.0.1:8080/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "input": "Hello, introduce yourself"
  }'
```

Streaming example:

```bash
curl http://127.0.0.1:8080/v1/responses \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "stream": true,
    "input": "Stream a short answer"
  }'
```

Full context example:

```json
{
  "model": "gpt-4.1-mini",
  "instructions": "You are a concise assistant.",
  "input": [
    {
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "Hello" }]
    },
    {
      "type": "message",
      "role": "assistant",
      "content": [{ "type": "output_text", "text": "Hi, how can I help?" }]
    },
    {
      "type": "message",
      "role": "user",
      "content": [{ "type": "input_text", "text": "Continue the previous topic" }]
    }
  ]
}
```

### `POST /v1/chat/completions`

Directly proxies to upstream `{upstream.url}/chat/completions`.

Use this route for clients that already send Chat Completions requests. In multi-upstream mode, OpenProxy reads the request `model` and sends the request to the matching upstream.

```bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "gpt-4.1-mini",
    "messages": [
      { "role": "user", "content": "Hello" }
    ]
  }'
```

### `GET /v1/models`

Proxies the upstream model list. In multi-upstream mode, model lists from all configured upstreams are merged.

```bash
curl http://127.0.0.1:8080/v1/models
```

## Configuration

| Field | Required | Default | Description |
| --- | --- | --- | --- |
| `port` | No | `8080` | Local listen port |
| `upstream.url` | Yes for legacy single-upstream config | None | Upstream API base URL, usually ending with `/v1` |
| `upstream.api_key` | No | Empty string | Upstream authentication key |
| `upstream.auth_header` | No | `Authorization` | Upstream authentication header name; empty values also use `Authorization` |
| `upstreams[].name` | No | None | Display/routing prefix for models, producing ids like `name:model` |
| `upstreams[].url` | Yes for multi-upstream config | None | Upstream API base URL for one provider |
| `upstreams[].api_key` | No | Empty string | Authentication key for one provider |
| `upstreams[].auth_header` | No | `Authorization` | Authentication header name for one provider |

## Conversion Flow

For `/v1/responses`:

1. Parse the incoming Responses API request.
2. Select the matching upstream by checking which configured upstream advertises the requested `model`.
3. Convert `instructions`, `input`, `tools`, `tool_choice`, and other fields into a Chat Completions-compatible request.
4. Send the converted request to upstream `{upstream.url}/chat/completions`.
5. Non-streaming response: parse Chat Completions JSON and convert it into Responses API JSON.
6. Streaming response: parse upstream SSE and emit Responses API SSE events.
7. Upstream error: retry when applicable, then pass through the final status code and error body.

## Development Commands

```bash
cargo fmt --check
cargo test
cargo clippy -- -D warnings
```

Common commands:

```bash
cargo fmt
cargo run
cargo test <test_name>
```

## Test Coverage

The test suite covers the main contract behaviors:

- Complete Responses input context conversion.
- Responses tool/tool_choice conversion to Chat Completions.
- Non-streaming tool_calls conversion to Responses function_call.
- Streaming text delta conversion.
- Streaming tool call argument delta/done events.
- SSE partial packet and multibyte character handling.
- Fallback output completion when `[DONE]` arrives without `finish_reason`.
- Retry for 403/429/5xx.
- `/v1/models` proxying and retry behavior.
- Multi-upstream `/v1/models` aggregation.
- Automatic upstream routing by requested model.
- OpenAI default `Authorization: Bearer <api_key>` authentication behavior.

## Notes

- Do not commit real `api_key` values to version control.
- If you use OpenAI-compatible authentication, you can omit `auth_header`.
- If your upstream uses a non-standard authentication header, set `auth_header` explicitly.
- Only `403`, `429`, `5xx`, and request send failures are retried. `401` usually means authentication failed and is not retried.
