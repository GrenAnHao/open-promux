//! `POST /v1/messages` — Anthropic Messages API as a downstream protocol.
//!
//! Lets clients that already speak Anthropic Messages (Claude SDKs, third
//! party apps that target `api.anthropic.com`) talk to any configured
//! upstream by pointing at open-promux's `/v1/messages`.
//!
//! Routing behaviour matches the other endpoints (model-aware upstream
//! selection, retry, optional failover, rate limiting, rectifier). The body
//! conversion strategy depends on the upstream's `api_format`:
//!
//! - `AnthropicMessages` upstream — direct passthrough (this module).
//! - `ChatCompletions` upstream — Anthropic ⇄ Chat translation. Currently
//!   non-streaming only; streaming returns `501 Not Implemented` with a
//!   helpful pointer until the SSE bridge lands.

use super::*;
use crate::convert;

pub async fn messages(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    if !is_proxy_authorized(&state.config, &parts.headers) {
        return unauthorized_response();
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[messages] failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let request_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let request_tokens = estimate_tokens_from_bytes(&body_bytes);
    if state
        .check_global_rate_limits(request_tokens)
        .await
        .is_err()
    {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }

    let is_stream = request_json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let model = request_json.get("model").and_then(|m| m.as_str());

    tracing::info!("[messages] POST /v1/messages model={model:?} stream={is_stream}");

    let selections = select_upstreams_for_model(&state, model).await;
    if selections.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "model not found in configured upstreams",
        )
            .into_response();
    }
    let last_selection_index = selections.len() - 1;

    for (selection_index, selection) in selections.into_iter().enumerate() {
        let can_failover =
            state.config.routing.automatic_failover && selection_index < last_selection_index;
        let upstream = selection.upstream;
        let upstream_config = &upstream.config;
        log_model_route("[messages]", &selection);

        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[messages] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        match upstream_config.api_format {
            UpstreamApiFormat::AnthropicMessages => {
                if let Some(response) = handle_anthropic_passthrough(
                    &state,
                    upstream,
                    &selection,
                    &parts.headers,
                    &request_json,
                    &body_bytes,
                    is_stream,
                    can_failover,
                    start,
                )
                .await
                {
                    return response;
                }
            }
            UpstreamApiFormat::ChatCompletions => {
                if is_stream {
                    tracing::warn!(
                        "[messages] ChatCompletions upstream + Anthropic streaming not yet supported"
                    );
                    return (
                        StatusCode::NOT_IMPLEMENTED,
                        "Streaming `/v1/messages` against a ChatCompletions upstream is not yet implemented. \
                         Use an `api_format = \"anthropic_messages\"` upstream, or call `/v1/responses` (which already bridges Responses → Chat / Anthropic in both directions).",
                    )
                        .into_response();
                }
                if let Some(response) = handle_chat_to_anthropic_non_streaming(
                    upstream,
                    &selection,
                    &request_json,
                    can_failover,
                    start,
                )
                .await
                {
                    return response;
                }
            }
        }
    }

    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response()
}

/// Direct passthrough to an Anthropic Messages upstream.
///
/// Returns `Some(response)` when the request reached a final outcome (success
/// streamed/buffered, or an error we want to bubble up). Returns `None` when
/// the caller should try the next upstream (failover path).
#[allow(clippy::too_many_arguments)]
async fn handle_anthropic_passthrough(
    state: &Arc<AppState>,
    upstream: &UpstreamState,
    selection: &UpstreamSelection<'_>,
    request_headers: &HeaderMap,
    request_json: &serde_json::Value,
    body_bytes: &[u8],
    is_stream: bool,
    can_failover: bool,
    start: Instant,
) -> Option<Response> {
    let upstream_config = &upstream.config;
    let upstream_url = upstream_config.url.trim_end_matches('/');
    let target = format!("{upstream_url}/messages");
    log_upstream_target("[messages]", upstream_config, &target);

    let upstream_body = build_anthropic_upstream_body(request_json, selection, body_bytes);

    let upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = match send_with_retries("[messages]", upstream_config, || {
        let mut builder = apply_anthropic_headers(apply_upstream_auth(
            upstream
                .client
                .post(&target)
                .header("content-type", "application/json"),
            upstream_config,
        ));

        // Forward client headers (anthropic-version, anthropic-beta, …)
        // except authentication and host which we must not leak / proxy.
        for (key, value) in request_headers.iter() {
            let name = key.as_str();
            if matches!(name, "host" | "authorization" | "x-api-key") {
                continue;
            }
            if let Ok(v) = value.to_str() {
                builder = builder.header(name, v);
            }
        }

        builder.body(upstream_body.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if can_failover {
                tracing::warn!("[messages] upstream request failed: {e}; failing over");
                return None;
            }
            tracing::error!("[messages] upstream request failed: {e}");
            return Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_upstream_status("[messages]", selection, status);

    if can_failover && should_retry_status(status) {
        tracing::warn!("[messages] upstream returned {status}; failing over");
        return None;
    }

    if status.is_client_error() || status.is_server_error() {
        let err_bytes = upstream_resp.bytes().await.unwrap_or_default();
        if let Some(retry_resp) = maybe_run_anthropic_rectifier_passthrough(
            state,
            upstream,
            selection,
            &target,
            &upstream_body,
            &err_bytes,
        )
        .await
        {
            return Some(retry_resp);
        }
        tracing::warn!(
            "[messages] upstream error: {}",
            String::from_utf8_lossy(&err_bytes)
        );
        dump_upstream_error_debug(
            "[messages]",
            status,
            upstream_config,
            &target,
            request_json,
            &upstream_body,
            &err_bytes,
        );
        let mut resp = Response::new(Body::from(err_bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Some(resp);
    }

    if is_stream {
        tracing::info!(
            "[messages] streaming response, elapsed={}ms",
            start.elapsed().as_millis()
        );
        let stream = upstream_resp.bytes_stream();
        let body = Body::from_stream(stream.map(move |r| {
            let _upstream_permit = &upstream_permit;
            r.map_err(std::io::Error::other)
        }));

        return Some(
            Response::builder()
                .status(status)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(body)
                .unwrap(),
        );
    }

    Some(match upstream_resp.bytes().await {
        Ok(bytes) => {
            tracing::info!(
                "[messages] done, {}B, elapsed={}ms",
                bytes.len(),
                start.elapsed().as_millis()
            );
            let mut resp = Response::new(Body::from(bytes));
            *resp.status_mut() = status;
            resp.headers_mut()
                .insert("content-type", "application/json".parse().unwrap());
            resp
        }
        Err(e) => {
            tracing::error!("[messages] failed to read upstream response: {e}");
            (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
        }
    })
}

/// Chat Completions upstream + non-streaming Anthropic downstream.
///
/// We reuse the existing `Responses` ⇄ Chat / Anthropic converters as a
/// proven middle layer: Anthropic request → Responses → Chat upstream →
/// Responses response → Anthropic response.
async fn handle_chat_to_anthropic_non_streaming(
    upstream: &UpstreamState,
    selection: &UpstreamSelection<'_>,
    request_json: &serde_json::Value,
    can_failover: bool,
    start: Instant,
) -> Option<Response> {
    let upstream_config = &upstream.config;

    let responses_req = match anthropic_value_to_responses_request(request_json) {
        Ok(req) => req,
        Err(message) => {
            tracing::error!("[messages] failed to translate anthropic request: {message}");
            return Some((StatusCode::BAD_REQUEST, message).into_response());
        }
    };

    let mut chat_req = convert::responses_to_chat(&responses_req);
    if let Some(upstream_model) = selection.upstream_model.as_ref() {
        chat_req.model = upstream_model.clone();
    }
    let chat_body = match serde_json::to_vec(&chat_req) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[messages] failed to serialize chat request: {e}");
            return Some((StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response());
        }
    };

    let upstream_url = upstream_config.url.trim_end_matches('/');
    let target = format!("{upstream_url}/chat/completions");
    log_upstream_target("[messages]", upstream_config, &target);

    let upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = match send_with_retries("[messages]", upstream_config, || {
        apply_upstream_auth(
            upstream
                .client
                .post(&target)
                .header("content-type", "application/json"),
            upstream_config,
        )
        .body(chat_body.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if can_failover {
                tracing::warn!("[messages] upstream request failed: {e}; failing over");
                return None;
            }
            tracing::error!("[messages] upstream request failed: {e}");
            return Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_upstream_status("[messages]", selection, status);

    if can_failover && should_retry_status(status) {
        tracing::warn!("[messages] upstream returned {status}; failing over");
        return None;
    }

    let bytes = match upstream_resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[messages] failed to read upstream response: {e}");
            return Some(
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response(),
            );
        }
    };

    if status.is_client_error() || status.is_server_error() {
        tracing::warn!(
            "[messages] upstream error: {}",
            String::from_utf8_lossy(&bytes)
        );
        dump_upstream_error_debug(
            "[messages]",
            status,
            upstream_config,
            &target,
            request_json,
            &chat_body,
            &bytes,
        );
        let mut resp = Response::new(Body::from(bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Some(resp);
    }

    let chat_resp: ChatResponse = match serde_json::from_slice(&bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[messages] failed to parse chat upstream response: {e}");
            return Some(
                (
                    StatusCode::BAD_GATEWAY,
                    format!("invalid upstream response: {e}"),
                )
                    .into_response(),
            );
        }
    };

    let anthropic_resp = chat_response_to_anthropic_value(&chat_resp);
    let out = serde_json::to_vec(&anthropic_resp).unwrap_or_default();

    drop(upstream_permit);

    tracing::info!(
        "[messages] done (chat→anthropic), {}B, elapsed={}ms",
        out.len(),
        start.elapsed().as_millis()
    );

    let mut resp = Response::new(Body::from(out));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    Some(resp)
}

/// Apply the rectifier to a body that's already in Anthropic format. Same
/// logic the responses endpoint uses, just without the extra Responses
/// hand-off — `/v1/messages` already speaks Anthropic.
async fn maybe_run_anthropic_rectifier_passthrough(
    state: &Arc<AppState>,
    upstream: &UpstreamState,
    selection: &UpstreamSelection<'_>,
    target: &str,
    upstream_body: &[u8],
    err_bytes: &[u8],
) -> Option<Response> {
    // Deserialize the bytes we already sent upstream into the typed
    // AnthropicRequest so we can run the existing rectifier helper. If the
    // body fails to parse we silently skip rectification — the original
    // error response will surface to the caller as-is.
    let anthropic_req: AnthropicRequest = serde_json::from_slice(upstream_body).ok()?;
    let retry_body =
        rectify_anthropic_retry_body(&state.config.rectifier, anthropic_req, err_bytes)?;
    tracing::info!("[messages] anthropic rectifier triggered; retrying same upstream");

    let upstream_config = &upstream.config;
    let retry_resp = send_with_retries("[messages]", upstream_config, || {
        apply_anthropic_headers(apply_upstream_auth(
            upstream
                .client
                .post(target)
                .header("content-type", "application/json"),
            upstream_config,
        ))
        .body(retry_body.clone())
    })
    .await
    .ok()?;

    let retry_status =
        StatusCode::from_u16(retry_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_upstream_status("[messages]", selection, retry_status);

    if retry_status.is_client_error() || retry_status.is_server_error() {
        let retry_err_bytes = retry_resp.bytes().await.ok()?;
        let mut resp = Response::new(Body::from(retry_err_bytes));
        *resp.status_mut() = retry_status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Some(resp);
    }

    let bytes = retry_resp.bytes().await.ok()?;
    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = retry_status;
    resp.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    Some(resp)
}

fn build_anthropic_upstream_body(
    request_json: &serde_json::Value,
    selection: &UpstreamSelection<'_>,
    body_bytes: &[u8],
) -> Vec<u8> {
    let Some(upstream_model) = selection.upstream_model.as_ref() else {
        return body_bytes.to_vec();
    };
    let mut req_json = request_json.clone();
    if let Some(obj) = req_json.as_object_mut() {
        obj.insert(
            "model".into(),
            serde_json::Value::String(upstream_model.clone()),
        );
    }
    serde_json::to_vec(&req_json).unwrap_or_else(|_| body_bytes.to_vec())
}

/// Convert a raw Anthropic Messages JSON value into a `ResponsesRequest` we
/// can run through the existing `responses_to_chat` converter. Returns a
/// human-readable error string when required fields are missing.
fn anthropic_value_to_responses_request(
    value: &serde_json::Value,
) -> Result<ResponsesRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "anthropic request must be a JSON object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|m| m.as_str())
        .ok_or_else(|| "anthropic request missing `model`".to_string())?
        .to_string();

    let stream = obj.get("stream").and_then(|v| v.as_bool());
    let temperature = obj.get("temperature").and_then(|v| v.as_f64());
    let top_p = obj.get("top_p").and_then(|v| v.as_f64());
    let max_output_tokens = obj
        .get("max_tokens")
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let instructions = obj
        .get("system")
        .and_then(|s| s.as_str())
        .map(ToString::to_string);

    // Build items as raw JSON first, then deserialize them into the typed
    // `ResponsesInputItem` shape. Using JSON values keeps the converter
    // small and lets serde handle the `#[serde(rename = "type")]` mapping.
    let mut input_items_json: Vec<serde_json::Value> = Vec::new();
    if let Some(messages) = obj.get("messages").and_then(|v| v.as_array()) {
        for message in messages {
            let Some(role) = message.get("role").and_then(|r| r.as_str()) else {
                continue;
            };
            let content = message.get("content").cloned().unwrap_or_default();
            input_items_json.extend(anthropic_message_to_responses_items(role, &content));
        }
    }
    let input_items: Vec<ResponsesInputItem> =
        serde_json::from_value(serde_json::Value::Array(input_items_json))
            .map_err(|e| format!("failed to translate anthropic messages: {e}"))?;

    let tools = obj.get("tools").and_then(|v| v.as_array()).map(|tools| {
        tools
            .iter()
            .filter_map(|tool| {
                let name = tool.get("name").and_then(|n| n.as_str())?;
                Some(ResponsesTool {
                    tool_type: "function".to_string(),
                    name: Some(name.to_string()),
                    description: tool
                        .get("description")
                        .and_then(|d| d.as_str())
                        .map(ToString::to_string),
                    parameters: tool.get("input_schema").cloned(),
                })
            })
            .collect()
    });

    let tool_choice = obj
        .get("tool_choice")
        .cloned()
        .map(translate_anthropic_tool_choice);

    Ok(ResponsesRequest {
        model,
        instructions,
        input: ResponsesInput::Items(input_items),
        stream,
        temperature,
        top_p,
        max_output_tokens,
        tools,
        tool_choice,
        extra: std::collections::HashMap::new(),
    })
}

fn anthropic_message_to_responses_items(
    role: &str,
    content: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let normalised_role = match role {
        "assistant" => "assistant",
        _ => "user",
    };

    match content {
        serde_json::Value::String(text) => vec![serde_json::json!({
            "type": "message",
            "role": normalised_role,
            "content": [{
                "type": if normalised_role == "assistant" { "output_text" } else { "input_text" },
                "text": text
            }]
        })],
        serde_json::Value::Array(parts) => {
            let mut items = Vec::new();
            let mut text_parts = Vec::new();

            for part in parts {
                let Some(part_type) = part.get("type").and_then(|t| t.as_str()) else {
                    continue;
                };
                match part_type {
                    "text" => {
                        if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                            text_parts.push(serde_json::json!({
                                "type": if normalised_role == "assistant" { "output_text" } else { "input_text" },
                                "text": text
                            }));
                        }
                    }
                    "tool_use" => {
                        // Flush pending text before a tool call so message order is preserved.
                        if !text_parts.is_empty() {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": normalised_role,
                                "content": std::mem::take(&mut text_parts)
                            }));
                        }
                        let id = part
                            .get("id")
                            .and_then(|id| id.as_str())
                            .unwrap_or_default();
                        let name = part
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default();
                        let input = part
                            .get("input")
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!({}));
                        let arguments =
                            serde_json::to_string(&input).unwrap_or_else(|_| "{}".into());
                        items.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": id,
                            "name": name,
                            "arguments": arguments
                        }));
                    }
                    "tool_result" => {
                        if !text_parts.is_empty() {
                            items.push(serde_json::json!({
                                "type": "message",
                                "role": normalised_role,
                                "content": std::mem::take(&mut text_parts)
                            }));
                        }
                        let call_id = part
                            .get("tool_use_id")
                            .and_then(|id| id.as_str())
                            .unwrap_or_default();
                        let output = match part.get("content") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(other) => other.to_string(),
                            None => String::new(),
                        };
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": output
                        }));
                    }
                    _ => {}
                }
            }

            if !text_parts.is_empty() {
                items.push(serde_json::json!({
                    "type": "message",
                    "role": normalised_role,
                    "content": text_parts
                }));
            }

            items
        }
        _ => Vec::new(),
    }
}

fn translate_anthropic_tool_choice(value: serde_json::Value) -> serde_json::Value {
    match value.get("type").and_then(|t| t.as_str()) {
        Some("any") => serde_json::Value::String("required".into()),
        Some("auto") => serde_json::Value::String("auto".into()),
        Some("tool") => {
            let name = value
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            serde_json::json!({"type": "function", "name": name})
        }
        _ => value,
    }
}

/// Convert an OpenAI Chat Completions response into an Anthropic Messages
/// response shape. Mirrors the inverse of `responses_to_anthropic` /
/// `anthropic_to_responses` plumbing but goes directly between the two
/// adjacent formats so the JSON round-trip stays minimal.
fn chat_response_to_anthropic_value(resp: &ChatResponse) -> serde_json::Value {
    let choice = resp.choices.first();
    let message = choice.map(|c| &c.message);
    let stop_reason = choice
        .and_then(|c| c.finish_reason.as_deref())
        .map(|reason| match reason {
            "tool_calls" => "tool_use",
            "length" => "max_tokens",
            "stop" => "end_turn",
            other => other,
        })
        .unwrap_or("end_turn");

    let mut content_blocks: Vec<serde_json::Value> = Vec::new();
    if let Some(message) = message {
        if let Some(text) = chat_message_text(&message.content)
            && !text.is_empty()
        {
            content_blocks.push(serde_json::json!({
                "type": "text",
                "text": text
            }));
        }
        if let Some(tool_calls) = message.tool_calls.as_ref() {
            for call in tool_calls {
                let input = serde_json::from_str::<serde_json::Value>(&call.function.arguments)
                    .unwrap_or_else(|_| serde_json::json!({}));
                content_blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": call.id,
                    "name": call.function.name,
                    "input": input
                }));
            }
        }
    }

    let usage = resp.usage.as_ref().map(|u| {
        serde_json::json!({
            "input_tokens": u.prompt_tokens,
            "output_tokens": u.completion_tokens,
        })
    });

    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::Value::String(resp.id.clone()));
    out.insert("type".into(), serde_json::Value::String("message".into()));
    out.insert("role".into(), serde_json::Value::String("assistant".into()));
    out.insert(
        "model".into(),
        serde_json::Value::String(resp.model.clone()),
    );
    out.insert("content".into(), serde_json::Value::Array(content_blocks));
    out.insert(
        "stop_reason".into(),
        serde_json::Value::String(stop_reason.into()),
    );
    if let Some(usage) = usage {
        out.insert("usage".into(), usage);
    }

    serde_json::Value::Object(out)
}

fn chat_message_text(content: &Option<String>) -> Option<String> {
    let text = content.as_ref()?;
    if text.is_empty() {
        None
    } else {
        Some(text.clone())
    }
}
