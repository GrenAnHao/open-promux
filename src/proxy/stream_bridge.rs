use super::*;
use axum::body::Bytes;
use futures::StreamExt;
use serde_json::{Map, Value, json};
use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

pub(super) fn stream_upstream_as_chat(
    upstream_resp: reqwest::Response,
    upstream_api_format: UpstreamApiFormat,
    model: String,
    upstream_permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Response {
    let stream = upstream_resp.bytes_stream();
    let transformed = futures::stream::unfold(
        (
            stream,
            convert::SseDecoder::new(),
            ChatSseEncoder::new(model),
            upstream_api_format,
            VecDeque::new(),
            false,
            None::<ChatUsage>,
        ),
        |(
            mut stream,
            mut decoder,
            mut encoder,
            upstream_api_format,
            mut pending,
            mut saw_tool_call,
            mut last_usage,
        )| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((
                        Ok::<_, std::io::Error>(event),
                        (
                            stream,
                            decoder,
                            encoder,
                            upstream_api_format,
                            pending,
                            saw_tool_call,
                            last_usage,
                        ),
                    ));
                }

                match stream.next().await {
                    Some(Ok(chunk_bytes)) => {
                        for data in decoder.push(&chunk_bytes) {
                            if data.trim() == "[DONE]" {
                                encoder.finish("stop", last_usage.clone(), &mut pending);
                                continue;
                            }

                            match upstream_api_format {
                                UpstreamApiFormat::AnthropicMessages => {
                                    let event: AnthropicStreamEvent = match serde_json::from_str(
                                        &data,
                                    ) {
                                        Ok(event) => event,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[stream-bridge] failed to parse anthropic event: {e}"
                                            );
                                            continue;
                                        }
                                    };
                                    if let Some(usage) =
                                        convert::anthropic_stream_event_usage(&event)
                                    {
                                        last_usage = Some(usage);
                                    }
                                    if let Some(chunk) =
                                        convert::anthropic_stream_event_to_chat_chunk(
                                            &event,
                                            encoder.model(),
                                        )
                                    {
                                        let mut chunk = chunk;
                                        if chunk.usage.is_none() {
                                            chunk.usage = last_usage.clone();
                                        }
                                        encoder.push_chunk(&chunk, &mut pending);
                                    }
                                    if convert::anthropic_stream_event_is_stop(&event) {
                                        encoder.finish("stop", last_usage.clone(), &mut pending);
                                    }
                                }
                                UpstreamApiFormat::Responses => {
                                    let value: Value = match serde_json::from_str(&data) {
                                        Ok(value) => value,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[stream-bridge] failed to parse responses event: {e}"
                                            );
                                            continue;
                                        }
                                    };
                                    let chunks = response_event_to_chat_chunks(
                                        &value,
                                        encoder.model(),
                                        &mut saw_tool_call,
                                    );
                                    for mut chunk in chunks {
                                        if let Some(usage) = chunk.usage.clone() {
                                            last_usage = Some(usage);
                                        } else {
                                            chunk.usage = last_usage.clone();
                                        }
                                        encoder.push_chunk(&chunk, &mut pending);
                                    }
                                }
                                UpstreamApiFormat::ChatCompletions => {}
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("[stream-bridge] upstream stream error: {e}");
                        return None;
                    }
                    None => {
                        encoder.finish("stop", last_usage.clone(), &mut pending);
                        if pending.is_empty() {
                            return None;
                        }
                    }
                }
            }
        },
    );

    let body = Body::from_stream(transformed.map(move |r| {
        let _upstream_permit = &upstream_permit;
        r
    }));

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

pub(super) fn stream_upstream_as_anthropic(
    upstream_resp: reqwest::Response,
    upstream_api_format: UpstreamApiFormat,
    model: String,
    upstream_permit: Option<tokio::sync::OwnedSemaphorePermit>,
) -> Response {
    let stream = upstream_resp.bytes_stream();
    let transformed = futures::stream::unfold(
        (
            stream,
            convert::SseDecoder::new(),
            AnthropicSseEncoder::new(model),
            upstream_api_format,
            VecDeque::new(),
            false,
            None::<ChatUsage>,
        ),
        |(
            mut stream,
            mut decoder,
            mut encoder,
            upstream_api_format,
            mut pending,
            mut saw_tool_call,
            mut last_usage,
        )| async move {
            loop {
                if let Some(event) = pending.pop_front() {
                    return Some((
                        Ok::<_, std::io::Error>(event),
                        (
                            stream,
                            decoder,
                            encoder,
                            upstream_api_format,
                            pending,
                            saw_tool_call,
                            last_usage,
                        ),
                    ));
                }

                match stream.next().await {
                    Some(Ok(chunk_bytes)) => {
                        for data in decoder.push(&chunk_bytes) {
                            if data.trim() == "[DONE]" {
                                encoder.finish("end_turn", last_usage.clone(), &mut pending);
                                continue;
                            }

                            match upstream_api_format {
                                UpstreamApiFormat::ChatCompletions => {
                                    let mut chunk: ChatChunk = match serde_json::from_str(&data) {
                                        Ok(chunk) => chunk,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[stream-bridge] failed to parse chat chunk: {e}"
                                            );
                                            continue;
                                        }
                                    };
                                    if let Some(usage) = chunk.usage.clone() {
                                        last_usage = Some(usage);
                                    } else {
                                        chunk.usage = last_usage.clone();
                                    }
                                    encoder.push_chunk(&chunk, &mut pending);
                                }
                                UpstreamApiFormat::Responses => {
                                    let value: Value = match serde_json::from_str(&data) {
                                        Ok(value) => value,
                                        Err(e) => {
                                            tracing::warn!(
                                                "[stream-bridge] failed to parse responses event: {e}"
                                            );
                                            continue;
                                        }
                                    };
                                    let chunks = response_event_to_chat_chunks(
                                        &value,
                                        encoder.model(),
                                        &mut saw_tool_call,
                                    );
                                    for mut chunk in chunks {
                                        if let Some(usage) = chunk.usage.clone() {
                                            last_usage = Some(usage);
                                        } else {
                                            chunk.usage = last_usage.clone();
                                        }
                                        encoder.push_chunk(&chunk, &mut pending);
                                    }
                                }
                                UpstreamApiFormat::AnthropicMessages => {}
                            }
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("[stream-bridge] upstream stream error: {e}");
                        return None;
                    }
                    None => {
                        encoder.finish("end_turn", last_usage.clone(), &mut pending);
                        if pending.is_empty() {
                            return None;
                        }
                    }
                }
            }
        },
    );

    let body = Body::from_stream(transformed.map(move |r| {
        let _upstream_permit = &upstream_permit;
        r
    }));

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

struct ChatSseEncoder {
    id: String,
    model: String,
    completed: bool,
}

impl ChatSseEncoder {
    fn new(model: String) -> Self {
        Self {
            id: stream_id("chatcmpl"),
            model,
            completed: false,
        }
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn push_chunk(&mut self, chunk: &ChatChunk, pending: &mut VecDeque<Bytes>) {
        if !chunk.model.is_empty() {
            self.model = chunk.model.clone();
        }
        let id = if chunk.id.is_empty() {
            self.id.clone()
        } else {
            chunk.id.clone()
        };
        for choice in &chunk.choices {
            let mut choice_value = Map::new();
            choice_value.insert("index".into(), json!(choice.index));
            choice_value.insert("delta".into(), chat_delta_value(&choice.delta));
            match &choice.finish_reason {
                Some(reason) => choice_value.insert("finish_reason".into(), json!(reason)),
                None => choice_value.insert("finish_reason".into(), Value::Null),
            };
            let mut payload = Map::new();
            payload.insert("id".into(), json!(id));
            payload.insert("object".into(), json!("chat.completion.chunk"));
            payload.insert("created".into(), json!(now_secs()));
            payload.insert("model".into(), json!(self.model));
            payload.insert(
                "choices".into(),
                Value::Array(vec![Value::Object(choice_value)]),
            );
            if let Some(usage) = chunk.usage.as_ref() {
                payload.insert("usage".into(), chat_usage_value(usage));
            }
            pending.push_back(data_sse(&Value::Object(payload)));
            if choice.finish_reason.is_some() {
                self.complete(pending);
            }
        }
    }

    fn finish(&mut self, reason: &str, usage: Option<ChatUsage>, pending: &mut VecDeque<Bytes>) {
        if self.completed {
            return;
        }
        let chunk = finish_chat_chunk(&self.id, &self.model, reason, usage);
        self.push_chunk(&chunk, pending);
    }

    fn complete(&mut self, pending: &mut VecDeque<Bytes>) {
        if self.completed {
            return;
        }
        pending.push_back(Bytes::from_static(b"data: [DONE]\n\n"));
        self.completed = true;
    }
}

struct AnthropicSseEncoder {
    id: String,
    model: String,
    started: bool,
    text_started: bool,
    completed: bool,
}

impl AnthropicSseEncoder {
    fn new(model: String) -> Self {
        Self {
            id: stream_id("msg"),
            model,
            started: false,
            text_started: false,
            completed: false,
        }
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn push_chunk(&mut self, chunk: &ChatChunk, pending: &mut VecDeque<Bytes>) {
        if !chunk.model.is_empty() {
            self.model = chunk.model.clone();
        }
        self.start(pending);
        for choice in &chunk.choices {
            if let Some(content) = choice.delta.content.as_ref() {
                self.start_text(pending);
                pending.push_back(event_sse(
                    "content_block_delta",
                    &json!({
                        "type": "content_block_delta",
                        "index": 0,
                        "delta": {"type": "text_delta", "text": content}
                    }),
                ));
            }
            if let Some(tool_calls) = choice.delta.tool_calls.as_ref() {
                for tool_call in tool_calls {
                    pending.push_back(event_sse(
                        "content_block_delta",
                        &json!({
                            "type": "content_block_delta",
                            "index": tool_call.index,
                            "delta": {
                                "type": "input_json_delta",
                                "partial_json": tool_call.function.as_ref().and_then(|f| f.arguments.as_ref()).cloned().unwrap_or_default()
                            }
                        }),
                    ));
                }
            }
            if let Some(reason) = choice.finish_reason.as_deref() {
                self.finish(anthropic_stop_reason(reason), chunk.usage.clone(), pending);
            }
        }
    }

    fn start(&mut self, pending: &mut VecDeque<Bytes>) {
        if self.started {
            return;
        }
        pending.push_back(event_sse(
            "message_start",
            &json!({
                "type": "message_start",
                "message": {
                    "id": self.id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0}
                }
            }),
        ));
        self.started = true;
    }

    fn start_text(&mut self, pending: &mut VecDeque<Bytes>) {
        if self.text_started {
            return;
        }
        pending.push_back(event_sse(
            "content_block_start",
            &json!({
                "type": "content_block_start",
                "index": 0,
                "content_block": {"type": "text", "text": ""}
            }),
        ));
        self.text_started = true;
    }

    fn finish(&mut self, reason: &str, usage: Option<ChatUsage>, pending: &mut VecDeque<Bytes>) {
        if self.completed {
            return;
        }
        self.start(pending);
        if self.text_started {
            pending.push_back(event_sse(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": 0}),
            ));
        }
        let usage = usage
            .as_ref()
            .map(anthropic_usage_value)
            .unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0}));
        pending.push_back(event_sse(
            "message_delta",
            &json!({
                "type": "message_delta",
                "delta": {"stop_reason": reason, "stop_sequence": null},
                "usage": usage
            }),
        ));
        pending.push_back(event_sse("message_stop", &json!({"type": "message_stop"})));
        self.completed = true;
    }
}

fn response_event_to_chat_chunks(
    value: &Value,
    model: &str,
    saw_tool_call: &mut bool,
) -> Vec<ChatChunk> {
    match value.get("type").and_then(Value::as_str) {
        Some("response.output_text.delta") => value
            .get("delta")
            .and_then(Value::as_str)
            .map(|text| vec![text_chat_chunk(model, text)])
            .unwrap_or_default(),
        Some("response.output_item.added") => {
            let Some(item) = value.get("item") else {
                return Vec::new();
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return Vec::new();
            }
            *saw_tool_call = true;
            let index = value
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            vec![tool_chat_chunk(
                model,
                index,
                item.get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str),
                item.get("name").and_then(Value::as_str),
                None,
            )]
        }
        Some("response.function_call_arguments.delta") => {
            *saw_tool_call = true;
            let index = value
                .get("output_index")
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            vec![tool_chat_chunk(
                model,
                index,
                None,
                None,
                value.get("delta").and_then(Value::as_str),
            )]
        }
        Some("response.completed") => {
            let response = value.get("response").unwrap_or(value);
            let usage = response
                .get("usage")
                .and_then(chat_usage_from_responses_usage);
            vec![finish_chat_chunk(
                "",
                response
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or(model),
                if *saw_tool_call { "tool_calls" } else { "stop" },
                usage,
            )]
        }
        _ => Vec::new(),
    }
}

fn text_chat_chunk(model: &str, content: &str) -> ChatChunk {
    ChatChunk {
        id: String::new(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta {
                role: None,
                content: Some(content.to_string()),
                reasoning_content: None,
                tool_calls: None,
            },
            finish_reason: None,
        }],
        usage: None,
    }
}

fn tool_chat_chunk(
    model: &str,
    index: u32,
    id: Option<&str>,
    name: Option<&str>,
    arguments: Option<&str>,
) -> ChatChunk {
    ChatChunk {
        id: String::new(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: Some(vec![ChatChunkToolCall {
                    index,
                    id: id.map(ToString::to_string),
                    call_type: Some("function".into()),
                    function: Some(ChatChunkFunction {
                        name: name.map(ToString::to_string),
                        arguments: arguments.map(ToString::to_string),
                    }),
                }]),
            },
            finish_reason: None,
        }],
        usage: None,
    }
}

fn finish_chat_chunk(id: &str, model: &str, reason: &str, usage: Option<ChatUsage>) -> ChatChunk {
    ChatChunk {
        id: id.to_string(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta: ChatChunkDelta {
                role: None,
                content: None,
                reasoning_content: None,
                tool_calls: None,
            },
            finish_reason: Some(reason.to_string()),
        }],
        usage,
    }
}

fn chat_delta_value(delta: &ChatChunkDelta) -> Value {
    let mut out = Map::new();
    if let Some(role) = delta.role.as_ref() {
        out.insert("role".into(), json!(role));
    }
    if let Some(content) = delta.content.as_ref() {
        out.insert("content".into(), json!(content));
    }
    if let Some(reasoning_content) = delta.reasoning_content.as_ref() {
        out.insert("reasoning_content".into(), json!(reasoning_content));
    }
    if let Some(tool_calls) = delta.tool_calls.as_ref() {
        out.insert(
            "tool_calls".into(),
            Value::Array(tool_calls.iter().map(chat_tool_call_delta_value).collect()),
        );
    }
    Value::Object(out)
}

fn chat_tool_call_delta_value(tool_call: &ChatChunkToolCall) -> Value {
    let mut out = Map::new();
    out.insert("index".into(), json!(tool_call.index));
    if let Some(id) = tool_call.id.as_ref() {
        out.insert("id".into(), json!(id));
    }
    if let Some(call_type) = tool_call.call_type.as_ref() {
        out.insert("type".into(), json!(call_type));
    }
    if let Some(function) = tool_call.function.as_ref() {
        let mut fn_out = Map::new();
        if let Some(name) = function.name.as_ref() {
            fn_out.insert("name".into(), json!(name));
        }
        if let Some(arguments) = function.arguments.as_ref() {
            fn_out.insert("arguments".into(), json!(arguments));
        }
        out.insert("function".into(), Value::Object(fn_out));
    }
    Value::Object(out)
}

fn chat_usage_from_responses_usage(value: &Value) -> Option<ChatUsage> {
    Some(ChatUsage {
        prompt_tokens: value.get("input_tokens").and_then(Value::as_u64)? as u32,
        completion_tokens: value.get("output_tokens").and_then(Value::as_u64)? as u32,
        total_tokens: value.get("total_tokens").and_then(Value::as_u64)? as u32,
    })
}

fn chat_usage_value(usage: &ChatUsage) -> Value {
    json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.total_tokens
    })
}

fn anthropic_usage_value(usage: &ChatUsage) -> Value {
    json!({
        "input_tokens": usage.prompt_tokens,
        "output_tokens": usage.completion_tokens
    })
}

fn anthropic_stop_reason(reason: &str) -> &str {
    match reason {
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        _ => "end_turn",
    }
}

fn data_sse(data: &Value) -> Bytes {
    Bytes::from(format!("data: {data}\n\n"))
}

fn event_sse(event: &str, data: &Value) -> Bytes {
    Bytes::from(format!("event: {event}\ndata: {data}\n\n"))
}

fn stream_id(prefix: &str) -> String {
    let uuid = uuid::Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{uuid}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
