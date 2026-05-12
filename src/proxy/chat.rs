use super::*;
use crate::convert;

pub async fn chat_completions(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    if !is_proxy_authorized(&state.config, &parts.headers) {
        return unauthorized_response();
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
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

    tracing::info!("[passthrough] POST /v1/chat/completions stream={is_stream}");

    if state.config.debug.enabled && state.config.debug.log_conversations {
        dump_conversation_debug("[chat]", &body_bytes);
    }

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
        log_model_route("[passthrough]", &selection);

        // Cross-format dispatch: chat downstream + non-chat upstream goes
        // through the Responses-API bridge (non-streaming only for now).
        if upstream_config.api_format != crate::config::UpstreamApiFormat::ChatCompletions {
            if is_stream {
                if let Some(response) = handle_chat_to_non_chat_streaming(
                    upstream,
                    &selection,
                    &request_json,
                    can_failover,
                )
                .await
                {
                    let response_status = response.status();
                    record_request_metric(
                        &state,
                        upstream_config,
                        model,
                        response_status.is_success(),
                        body_bytes.len() as u64,
                        0,
                        start.elapsed().as_millis() as u64,
                    )
                    .await;
                    return response;
                }
                continue;
            }
            if let Some(response) = handle_chat_to_non_chat_non_streaming(
                &state,
                upstream,
                &selection,
                &request_json,
                &body_bytes,
                can_failover,
                start,
            )
            .await
            {
                let response_status = response.status();
                record_request_metric(
                    &state,
                    upstream_config,
                    model,
                    response_status.is_success(),
                    body_bytes.len() as u64,
                    0,
                    start.elapsed().as_millis() as u64,
                )
                .await;
                return response;
            }
            continue;
        }

        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[passthrough] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/chat/completions");
        log_upstream_target("[passthrough]", upstream_config, &target);
        let upstream_body = if let Some(upstream_model) = selection.upstream_model.as_ref() {
            let mut upstream_json = request_json.clone();
            if let Some(obj) = upstream_json.as_object_mut() {
                obj.insert(
                    "model".into(),
                    serde_json::Value::String(upstream_model.clone()),
                );
            }
            serde_json::to_vec(&upstream_json).unwrap_or_else(|_| body_bytes.to_vec())
        } else {
            body_bytes.to_vec()
        };

        let upstream_permit = upstream.acquire_permit().await;
        let upstream_resp = match send_with_retries("[passthrough]", upstream_config, || {
            let mut builder = apply_upstream_auth(
                upstream
                    .client
                    .post(&target)
                    .header("content-type", "application/json"),
                upstream_config,
            );

            for (key, value) in parts.headers.iter() {
                if key == "host" || key == "authorization" {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            builder.body(upstream_body.clone())
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if can_failover {
                    tracing::warn!("[passthrough] upstream request failed: {e}; failing over");
                    continue;
                }
                tracing::error!("[passthrough] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!(
            "[passthrough] upstream protocol: {:?}",
            upstream_resp.version()
        );

        log_upstream_status("[passthrough]", &selection, status);

        if can_failover && should_retry_status(status) {
            tracing::warn!("[passthrough] upstream returned {status}; failing over");
            continue;
        }

        if is_stream && status.is_success() {
            tracing::info!(
                "[passthrough] streaming response, elapsed={}ms",
                start.elapsed().as_millis()
            );
            record_request_metric(
                &state,
                upstream_config,
                model,
                true,
                body_bytes.len() as u64,
                0,
                start.elapsed().as_millis() as u64,
            )
            .await;
            let stream = upstream_resp.bytes_stream();
            let body = Body::from_stream(stream.map(move |r| {
                let _upstream_permit = &upstream_permit;
                r.map_err(std::io::Error::other)
            }));

            return Response::builder()
                .status(status)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(body)
                .unwrap();
        }

        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                if status.is_client_error() || status.is_server_error() {
                    tracing::warn!(
                        "[passthrough] upstream error: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                    dump_upstream_error_debug(
                        "[passthrough]",
                        status,
                        upstream_config,
                        &target,
                        &request_json,
                        &upstream_body,
                        &bytes,
                    );
                }
                tracing::info!(
                    "[passthrough] done, {}B, elapsed={}ms",
                    bytes.len(),
                    start.elapsed().as_millis()
                );
                record_request_metric(
                    &state,
                    upstream_config,
                    model,
                    status.is_success(),
                    body_bytes.len() as u64,
                    bytes.len() as u64,
                    start.elapsed().as_millis() as u64,
                )
                .await;
                let mut resp = Response::new(Body::from(bytes));
                *resp.status_mut() = status;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                resp
            }
            Err(e) => {
                tracing::error!("[passthrough] failed to read upstream response: {e}");
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
            }
        };
    }

    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response()
}

async fn handle_chat_to_non_chat_streaming(
    upstream: &UpstreamState,
    selection: &UpstreamSelection<'_>,
    request_json: &serde_json::Value,
    can_failover: bool,
) -> Option<Response> {
    let upstream_config = &upstream.config;
    let mut responses_req = match chat_value_to_responses_request(request_json) {
        Ok(req) => req,
        Err(message) => {
            tracing::error!("[passthrough] failed to translate chat request: {message}");
            return Some((StatusCode::BAD_REQUEST, message).into_response());
        }
    };
    responses_req.stream = Some(true);
    if let Some(upstream_model) = selection.upstream_model.as_ref() {
        responses_req.model = upstream_model.clone();
    }

    let upstream_url = upstream_config.url.trim_end_matches('/');
    let (target, upstream_body, is_anthropic) = match upstream_config.api_format {
        crate::config::UpstreamApiFormat::AnthropicMessages => {
            let mut anthropic_req = convert::responses_to_anthropic(&responses_req);
            anthropic_req.stream = Some(true);
            let body = match serde_json::to_vec(&anthropic_req) {
                Ok(body) => body,
                Err(e) => {
                    tracing::error!("[passthrough] failed to serialize anthropic request: {e}");
                    return Some(
                        (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response(),
                    );
                }
            };
            (format!("{upstream_url}/messages"), body, true)
        }
        crate::config::UpstreamApiFormat::Responses => {
            let body = match serde_json::to_vec(&responses_req) {
                Ok(body) => body,
                Err(e) => {
                    tracing::error!("[passthrough] failed to serialize responses request: {e}");
                    return Some(
                        (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response(),
                    );
                }
            };
            (format!("{upstream_url}/responses"), body, false)
        }
        crate::config::UpstreamApiFormat::ChatCompletions => {
            unreachable!("handle_chat_to_non_chat_streaming called with chat upstream");
        }
    };
    log_upstream_target("[passthrough]", upstream_config, &target);

    let upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = match send_with_retries("[passthrough]", upstream_config, || {
        let builder = apply_upstream_auth(
            upstream
                .client
                .post(&target)
                .header("content-type", "application/json"),
            upstream_config,
        );
        let builder = if is_anthropic {
            apply_anthropic_headers(builder)
        } else {
            builder
        };
        builder.body(upstream_body.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if can_failover {
                tracing::warn!("[passthrough] upstream request failed: {e}; failing over");
                return None;
            }
            tracing::error!("[passthrough] upstream request failed: {e}");
            return Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_upstream_status("[passthrough]", selection, status);

    if can_failover && should_retry_status(status) {
        tracing::warn!("[passthrough] upstream returned {status}; failing over");
        return None;
    }

    if status.is_client_error() || status.is_server_error() {
        let bytes = upstream_resp.bytes().await.unwrap_or_default();
        tracing::warn!(
            "[passthrough] upstream error: {}",
            String::from_utf8_lossy(&bytes)
        );
        dump_upstream_error_debug(
            "[passthrough]",
            status,
            upstream_config,
            &target,
            request_json,
            &upstream_body,
            &bytes,
        );
        let mut resp = Response::new(Body::from(bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Some(resp);
    }

    Some(stream_upstream_as_chat(
        upstream_resp,
        upstream_config.api_format,
        responses_req.model,
        upstream_permit,
    ))
}

/// Chat downstream + non-Chat upstream (Anthropic Messages or Responses)
/// for non-streaming requests. Uses the Responses-API converters as a
/// shared middle layer so we don't have to write a fourth pair of
/// chat ⇄ N converters.
async fn handle_chat_to_non_chat_non_streaming(
    _state: &Arc<AppState>,
    upstream: &UpstreamState,
    selection: &UpstreamSelection<'_>,
    request_json: &serde_json::Value,
    request_body: &[u8],
    can_failover: bool,
    start: Instant,
) -> Option<Response> {
    let upstream_config = &upstream.config;

    // 1. Translate downstream chat-shape JSON → ResponsesRequest middle form.
    let mut responses_req = match chat_value_to_responses_request(request_json) {
        Ok(req) => req,
        Err(message) => {
            tracing::error!("[passthrough] failed to translate chat request: {message}");
            return Some((StatusCode::BAD_REQUEST, message).into_response());
        }
    };
    if let Some(upstream_model) = selection.upstream_model.as_ref() {
        responses_req.model = upstream_model.clone();
    }

    // 2. Build the upstream-specific body / target.
    let upstream_url = upstream_config.url.trim_end_matches('/');
    let (target, upstream_body, is_anthropic) = match upstream_config.api_format {
        crate::config::UpstreamApiFormat::AnthropicMessages => {
            let anthropic_req = convert::responses_to_anthropic(&responses_req);
            let body = match serde_json::to_vec(&anthropic_req) {
                Ok(body) => body,
                Err(e) => {
                    tracing::error!("[passthrough] failed to serialize anthropic request: {e}");
                    return Some(
                        (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response(),
                    );
                }
            };
            (format!("{upstream_url}/messages"), body, true)
        }
        crate::config::UpstreamApiFormat::Responses => {
            let body = match serde_json::to_vec(&responses_req) {
                Ok(body) => body,
                Err(e) => {
                    tracing::error!("[passthrough] failed to serialize responses request: {e}");
                    return Some(
                        (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response(),
                    );
                }
            };
            (format!("{upstream_url}/responses"), body, false)
        }
        crate::config::UpstreamApiFormat::ChatCompletions => {
            // Caller guarantees the upstream is non-chat here.
            unreachable!("handle_chat_to_non_chat_non_streaming called with chat upstream");
        }
    };
    log_upstream_target("[passthrough]", upstream_config, &target);

    // 3. Send upstream with the right auth + anthropic header.
    let _upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = match send_with_retries("[passthrough]", upstream_config, || {
        let builder = apply_upstream_auth(
            upstream
                .client
                .post(&target)
                .header("content-type", "application/json"),
            upstream_config,
        );
        let builder = if is_anthropic {
            apply_anthropic_headers(builder)
        } else {
            builder
        };
        builder.body(upstream_body.clone())
    })
    .await
    {
        Ok(r) => r,
        Err(e) => {
            if can_failover {
                tracing::warn!("[passthrough] upstream request failed: {e}; failing over");
                return None;
            }
            tracing::error!("[passthrough] upstream request failed: {e}");
            return Some((StatusCode::BAD_GATEWAY, "upstream request failed").into_response());
        }
    };

    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    log_upstream_status("[passthrough]", selection, status);

    if can_failover && should_retry_status(status) {
        tracing::warn!("[passthrough] upstream returned {status}; failing over");
        return None;
    }

    let bytes = match upstream_resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[passthrough] failed to read upstream response: {e}");
            return Some(
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response(),
            );
        }
    };

    if status.is_client_error() || status.is_server_error() {
        tracing::warn!(
            "[passthrough] upstream error: {}",
            String::from_utf8_lossy(&bytes)
        );
        dump_upstream_error_debug(
            "[passthrough]",
            status,
            upstream_config,
            &target,
            request_json,
            request_body,
            &bytes,
        );
        let mut resp = Response::new(Body::from(bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return Some(resp);
    }

    // 4. Translate the upstream's response back to a ChatCompletion.
    let chat_resp = if is_anthropic {
        let anthropic_resp: AnthropicResponse = match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[passthrough] failed to parse anthropic response: {e}");
                return Some(
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("invalid upstream response: {e}"),
                    )
                        .into_response(),
                );
            }
        };
        let responses_resp = convert::anthropic_to_responses(&anthropic_resp);
        responses_response_to_chat_value(&responses_resp)
    } else {
        let responses_value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!("[passthrough] failed to parse responses response: {e}");
                return Some(
                    (
                        StatusCode::BAD_GATEWAY,
                        format!("invalid upstream response: {e}"),
                    )
                        .into_response(),
                );
            }
        };
        responses_value_to_chat_value(&responses_value)
    };

    let out = serde_json::to_vec(&chat_resp).unwrap_or_default();
    tracing::info!(
        "[passthrough] done ({}→chat), {}B, elapsed={}ms",
        if is_anthropic {
            "anthropic"
        } else {
            "responses"
        },
        out.len(),
        start.elapsed().as_millis()
    );

    let mut resp = Response::new(Body::from(out));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    Some(resp)
}

/// Translate a chat-completions-shaped JSON request into a
/// [`ResponsesRequest`]. We accept the JSON-Value form so the caller
/// doesn't have to fully deserialise the body to a `ChatRequest`.
fn chat_value_to_responses_request(value: &serde_json::Value) -> Result<ResponsesRequest, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "chat request must be a JSON object".to_string())?;

    let model = obj
        .get("model")
        .and_then(|m| m.as_str())
        .ok_or_else(|| "chat request missing `model`".to_string())?
        .to_string();
    let stream = obj.get("stream").and_then(|v| v.as_bool());
    let temperature = obj.get("temperature").and_then(|v| v.as_f64());
    let top_p = obj.get("top_p").and_then(|v| v.as_f64());
    let max_output_tokens = obj
        .get("max_tokens")
        .or_else(|| obj.get("max_completion_tokens"))
        .and_then(|v| v.as_u64())
        .map(|n| n as u32);

    let mut instructions_parts: Vec<String> = Vec::new();
    let mut items_json: Vec<serde_json::Value> = Vec::new();
    if let Some(messages) = obj.get("messages").and_then(|v| v.as_array()) {
        for message in messages {
            let role = message
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user");
            match role {
                "system" | "developer" => {
                    if let Some(text) = message.get("content").and_then(|c| c.as_str()) {
                        instructions_parts.push(text.to_string());
                    }
                }
                "tool" => {
                    let call_id = message
                        .get("tool_call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default();
                    let output = match message.get("content") {
                        Some(serde_json::Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                        None => String::new(),
                    };
                    items_json.push(serde_json::json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": output
                    }));
                }
                "assistant" => {
                    if let Some(tool_calls) = message.get("tool_calls").and_then(|c| c.as_array()) {
                        for tool_call in tool_calls {
                            let call_id = tool_call
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let fn_obj = tool_call.get("function");
                            let name = fn_obj
                                .and_then(|f| f.get("name"))
                                .and_then(|v| v.as_str())
                                .unwrap_or_default();
                            let arguments = fn_obj
                                .and_then(|f| f.get("arguments"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("{}");
                            items_json.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": call_id,
                                "name": name,
                                "arguments": arguments
                            }));
                        }
                    }
                    let content = message.get("content").cloned().unwrap_or_default();
                    items_json.extend(chat_role_content_to_responses_items("assistant", &content));
                }
                _ => {
                    let content = message.get("content").cloned().unwrap_or_default();
                    items_json.extend(chat_role_content_to_responses_items("user", &content));
                }
            }
        }
    }

    let input_items: Vec<ResponsesInputItem> =
        serde_json::from_value(serde_json::Value::Array(items_json))
            .map_err(|e| format!("failed to translate chat messages: {e}"))?;

    Ok(ResponsesRequest {
        model,
        instructions: if instructions_parts.is_empty() {
            None
        } else {
            Some(instructions_parts.join("\n\n"))
        },
        input: ResponsesInput::Items(input_items),
        stream,
        temperature,
        top_p,
        max_output_tokens,
        tools: obj.get("tools").and_then(|t| t.as_array()).map(|tools| {
            tools
                .iter()
                .filter_map(|tool| {
                    let function = tool.get("function")?;
                    let name = function.get("name").and_then(|n| n.as_str())?;
                    Some(ResponsesTool {
                        tool_type: "function".to_string(),
                        name: Some(name.to_string()),
                        description: function
                            .get("description")
                            .and_then(|d| d.as_str())
                            .map(ToString::to_string),
                        parameters: function.get("parameters").cloned(),
                    })
                })
                .collect()
        }),
        tool_choice: obj.get("tool_choice").cloned(),
        extra: std::collections::HashMap::new(),
    })
}

fn chat_role_content_to_responses_items(
    role: &str,
    content: &serde_json::Value,
) -> Vec<serde_json::Value> {
    let part_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    match content {
        serde_json::Value::String(text) if !text.is_empty() => vec![serde_json::json!({
            "type": "message",
            "role": role,
            "content": [{"type": part_type, "text": text}]
        })],
        serde_json::Value::Array(parts) => {
            let collected: Vec<serde_json::Value> = parts
                .iter()
                .filter_map(|part| {
                    let kind = part.get("type").and_then(|t| t.as_str())?;
                    let text = part.get("text").and_then(|v| v.as_str())?;
                    Some(serde_json::json!({
                        "type": if kind == "text" { part_type } else { kind },
                        "text": text
                    }))
                })
                .collect();
            if collected.is_empty() {
                Vec::new()
            } else {
                vec![serde_json::json!({
                    "type": "message",
                    "role": role,
                    "content": collected
                })]
            }
        }
        _ => Vec::new(),
    }
}

/// Convert a `ResponsesResponse` (typed) into a ChatCompletion-shaped JSON
/// value. Inverse of `responses_to_chat` (which goes request-side).
fn responses_response_to_chat_value(resp: &ResponsesResponse) -> serde_json::Value {
    let mut content_buf = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let mut reasoning: Option<String> = None;
    let mut finish_reason = "stop".to_string();

    for item in &resp.output {
        match item {
            ResponseOutputItem::Message {
                content,
                reasoning_content,
                ..
            } => {
                if let Some(r) = reasoning_content {
                    reasoning = Some(r.clone());
                }
                for part in content {
                    content_buf.push_str(&part.text);
                }
            }
            ResponseOutputItem::FunctionCall {
                call_id,
                name,
                arguments,
                ..
            } => {
                finish_reason = "tool_calls".to_string();
                tool_calls.push(serde_json::json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments,
                    }
                }));
            }
        }
    }

    let mut message = serde_json::Map::new();
    message.insert("role".into(), serde_json::Value::String("assistant".into()));
    if content_buf.is_empty() {
        message.insert("content".into(), serde_json::Value::Null);
    } else {
        message.insert("content".into(), serde_json::Value::String(content_buf));
    }
    if let Some(r) = reasoning {
        message.insert("reasoning_content".into(), serde_json::Value::String(r));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), serde_json::Value::Array(tool_calls));
    }

    serde_json::json!({
        "id": resp.id,
        "object": "chat.completion",
        "created": resp.created_at,
        "model": resp.model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": {
            "prompt_tokens": resp.usage.input_tokens,
            "completion_tokens": resp.usage.output_tokens,
            "total_tokens": resp.usage.total_tokens,
        }
    })
}

/// Same shape as [`responses_response_to_chat_value`] but accepts the raw
/// JSON value (used when the Responses upstream emits arbitrary fields we
/// don't model in our `ResponsesResponse` struct).
fn responses_value_to_chat_value(resp: &serde_json::Value) -> serde_json::Value {
    let id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("chatcmpl_unknown")
        .to_string();
    let model = resp
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let created = resp.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0);

    let mut content_buf = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let mut reasoning: Option<String> = None;
    let mut finish_reason = "stop".to_string();

    if let Some(items) = resp.get("output").and_then(|v| v.as_array()) {
        for item in items {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("message") => {
                    if let Some(r) = item.get("reasoning_content").and_then(|r| r.as_str()) {
                        reasoning = Some(r.to_string());
                    }
                    if let Some(parts) = item.get("content").and_then(|c| c.as_array()) {
                        for part in parts {
                            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                content_buf.push_str(text);
                            }
                        }
                    }
                }
                Some("function_call") => {
                    finish_reason = "tool_calls".to_string();
                    tool_calls.push(serde_json::json!({
                        "id": item.get("call_id").or_else(|| item.get("id")).cloned().unwrap_or_default(),
                        "type": "function",
                        "function": {
                            "name": item.get("name").cloned().unwrap_or_default(),
                            "arguments": item.get("arguments").cloned().unwrap_or_else(|| serde_json::Value::String("{}".into())),
                        }
                    }));
                }
                _ => {}
            }
        }
    }

    let mut message = serde_json::Map::new();
    message.insert("role".into(), serde_json::Value::String("assistant".into()));
    if content_buf.is_empty() {
        message.insert("content".into(), serde_json::Value::Null);
    } else {
        message.insert("content".into(), serde_json::Value::String(content_buf));
    }
    if let Some(r) = reasoning {
        message.insert("reasoning_content".into(), serde_json::Value::String(r));
    }
    if !tool_calls.is_empty() {
        message.insert("tool_calls".into(), serde_json::Value::Array(tool_calls));
    }

    let usage = resp.get("usage").cloned().unwrap_or_else(
        || serde_json::json!({"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0}),
    );
    // Normalise input_tokens/output_tokens → prompt_tokens/completion_tokens
    // so downstream ChatCompletion clients see expected keys.
    let usage = if let Some(obj) = usage.as_object() {
        let prompt = obj
            .get("prompt_tokens")
            .or_else(|| obj.get("input_tokens"))
            .cloned()
            .unwrap_or_else(|| serde_json::Value::from(0));
        let completion = obj
            .get("completion_tokens")
            .or_else(|| obj.get("output_tokens"))
            .cloned()
            .unwrap_or_else(|| serde_json::Value::from(0));
        let total = obj
            .get("total_tokens")
            .cloned()
            .unwrap_or_else(|| serde_json::Value::from(0));
        serde_json::json!({
            "prompt_tokens": prompt,
            "completion_tokens": completion,
            "total_tokens": total,
        })
    } else {
        usage
    };

    serde_json::json!({
        "id": id,
        "object": "chat.completion",
        "created": created,
        "model": model,
        "choices": [{
            "index": 0,
            "message": message,
            "finish_reason": finish_reason,
        }],
        "usage": usage,
    })
}
