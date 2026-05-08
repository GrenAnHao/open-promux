use crate::types::*;
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

fn gen_id(prefix: &str) -> String {
    let uuid = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{uuid}")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn responses_content_to_chat_content(
    content: Option<serde_json::Value>,
) -> Option<serde_json::Value> {
    match content? {
        serde_json::Value::Array(parts) => {
            let mut text = String::new();
            let mut all_text = true;

            for part in &parts {
                let part_type = part.get("type").and_then(|v| v.as_str());
                let part_text = part.get("text").and_then(|v| v.as_str());

                match (part_type, part_text) {
                    (Some("input_text" | "output_text"), Some(value)) => text.push_str(value),
                    _ => {
                        all_text = false;
                        break;
                    }
                }
            }

            if all_text {
                Some(serde_json::Value::String(text))
            } else {
                Some(serde_json::Value::Array(
                    parts.into_iter().map(map_responses_content_part).collect(),
                ))
            }
        }
        other => Some(other),
    }
}

fn map_responses_content_part(part: serde_json::Value) -> serde_json::Value {
    if let serde_json::Value::Object(obj) = &part {
        match obj.get("type").and_then(|v| v.as_str()) {
            Some("input_text" | "output_text") => {
                return serde_json::json!({
                    "type": "text",
                    "text": obj.get("text").cloned().unwrap_or_else(|| serde_json::Value::String(String::new()))
                });
            }
            Some("input_image") => {
                if let Some(url) = obj.get("image_url").or_else(|| obj.get("url")) {
                    return serde_json::json!({
                        "type": "image_url",
                        "image_url": {"url": url}
                    });
                }
            }
            _ => {}
        }
    }

    part
}

fn chat_compatible_extra(
    extra: &HashMap<String, serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    const KEYS: &[&str] = &[
        "frequency_penalty",
        "logit_bias",
        "logprobs",
        "max_completion_tokens",
        "metadata",
        "n",
        "parallel_tool_calls",
        "presence_penalty",
        "response_format",
        "seed",
        "stop",
        "stream_options",
        "top_logprobs",
        "user",
    ];

    extra
        .iter()
        .filter(|(key, _)| KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn chat_compatible_tool_choice(
    tool_choice: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let value = tool_choice?;

    if value.get("function").is_some() {
        return Some(value.clone());
    }

    if value.get("type").and_then(|v| v.as_str()) == Some("function")
        && let Some(name) = value.get("name").and_then(|v| v.as_str())
    {
        return Some(serde_json::json!({
            "type": "function",
            "function": {
                "name": name
            }
        }));
    }

    Some(value.clone())
}

// ─── Responses → ChatCompletions ───

pub fn responses_to_chat(req: &ResponsesRequest) -> ChatRequest {
    let mut messages: Vec<ChatMessage> = Vec::new();

    // instructions → system message
    if let Some(ref instructions) = req.instructions {
        messages.push(ChatMessage {
            role: "system".into(),
            content: Some(serde_json::Value::String(instructions.clone())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
        });
    }

    // input → messages
    match &req.input {
        ResponsesInput::String(s) => {
            messages.push(ChatMessage {
                role: "user".into(),
                content: Some(serde_json::Value::String(s.clone())),
                tool_calls: None,
                tool_call_id: None,
                name: None,
            });
        }
        ResponsesInput::Items(items) => {
            for item in items {
                match item.item_type.as_deref() {
                    Some("function_call") => {
                        // assistant tool call — add as assistant message with tool_calls
                        messages.push(ChatMessage {
                            role: "assistant".into(),
                            content: None,
                            tool_calls: Some(vec![ChatToolCall {
                                id: item.call_id.clone().unwrap_or_default(),
                                call_type: "function".into(),
                                function: ChatFunction {
                                    name: item.name.clone().unwrap_or_default(),
                                    arguments: item.arguments.clone().unwrap_or_default(),
                                },
                            }]),
                            tool_call_id: None,
                            name: None,
                        });
                    }
                    Some("function_call_output") => {
                        messages.push(ChatMessage {
                            role: "tool".into(),
                            content: Some(serde_json::Value::String(
                                item.output.clone().unwrap_or_default(),
                            )),
                            tool_calls: None,
                            tool_call_id: item.call_id.clone(),
                            name: None,
                        });
                    }
                    _ => {
                        // regular message (user/assistant)
                        let role = item.role.as_deref().unwrap_or("user");
                        messages.push(ChatMessage {
                            role: role.into(),
                            content: responses_content_to_chat_content(item.content.clone()),
                            tool_calls: None,
                            tool_call_id: None,
                            name: None,
                        });
                    }
                }
            }
        }
    }

    // tools: Responses format → ChatCompletions format
    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter(|t| t.tool_type == "function")
            .map(|t| ChatTool {
                tool_type: "function".into(),
                function: ChatToolFunction {
                    name: t.name.clone().unwrap_or_default(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    });

    ChatRequest {
        model: req.model.clone(),
        messages,
        stream: req.stream,
        temperature: req.temperature,
        max_tokens: req.max_output_tokens,
        top_p: req.top_p,
        tools,
        tool_choice: chat_compatible_tool_choice(req.tool_choice.as_ref()),
        extra: chat_compatible_extra(&req.extra),
    }
}

// ─── ChatCompletions → Responses (non-streaming) ───

pub fn chat_to_responses(resp: &ChatResponse) -> ResponsesResponse {
    let resp_id = gen_id("resp");
    let mut output: Vec<ResponseOutputItem> = Vec::new();

    for choice in &resp.choices {
        // tool_calls → function_call items
        if let Some(ref tool_calls) = choice.message.tool_calls {
            for tc in tool_calls {
                output.push(ResponseOutputItem::FunctionCall {
                    id: gen_id("fc"),
                    status: "completed".into(),
                    call_id: tc.id.clone(),
                    name: tc.function.name.clone(),
                    arguments: tc.function.arguments.clone(),
                });
            }
        }

        // text content → message item
        if let Some(ref content) = choice.message.content
            && !content.is_empty()
        {
            output.push(ResponseOutputItem::Message {
                id: gen_id("msg"),
                status: "completed".into(),
                role: choice.message.role.clone(),
                content: vec![ResponseContentPart {
                    part_type: "output_text".into(),
                    text: content.clone(),
                    annotations: vec![],
                }],
            });
        }
    }

    let usage = resp
        .usage
        .as_ref()
        .map(|u| ResponsesUsage {
            input_tokens: u.prompt_tokens,
            output_tokens: u.completion_tokens,
            total_tokens: u.total_tokens,
        })
        .unwrap_or(ResponsesUsage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
        });

    ResponsesResponse {
        id: resp_id,
        object: "response".into(),
        created_at: now_secs(),
        model: resp.model.clone(),
        status: "completed".into(),
        output,
        usage,
    }
}

// ─── Stream state ───

pub struct StreamState {
    pub response_id: String,
    pub message_item_id: String,
    pub model: String,
    pub started: bool,
    pub content_started: bool,
    pub message_done: bool,
    pub message_output_index: Option<usize>,
    pub accumulated_text: String,
    pub current_tool_calls: BTreeMap<u32, StreamToolCall>,
    pub output_items: BTreeMap<usize, serde_json::Value>,
    next_output_index: usize,
}

pub struct StreamToolCall {
    pub call_id: String,
    pub id: String,
    pub name: String,
    pub arguments: String,
    pub output_index: Option<usize>,
    pub added: bool,
    pub done: bool,
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            response_id: gen_id("resp"),
            message_item_id: gen_id("msg"),
            model: String::new(),
            started: false,
            content_started: false,
            message_done: false,
            message_output_index: None,
            accumulated_text: String::new(),
            current_tool_calls: BTreeMap::new(),
            output_items: BTreeMap::new(),
            next_output_index: 0,
        }
    }

    fn allocate_output_index(&mut self) -> usize {
        let output_index = self.next_output_index;
        self.next_output_index += 1;
        output_index
    }
}

pub fn convert_stream_start(state: &mut StreamState, model: &str) -> Vec<String> {
    let mut events = Vec::new();

    // response.created
    events.push(format_sse(
        "response.created",
        &serde_json::json!({
            "type": "response.created",
            "response": {
                "id": state.response_id,
                "object": "response",
                "created_at": now_secs(),
                "model": model,
                "status": "in_progress",
                "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}
            }
        }),
    ));

    // response.in_progress
    events.push(format_sse(
        "response.in_progress",
        &serde_json::json!({
            "type": "response.in_progress",
            "response": {
                "id": state.response_id,
                "object": "response",
                "status": "in_progress",
                "output": []
            }
        }),
    ));

    state.started = true;
    state.model = model.into();
    events
}

pub fn convert_stream_chunk(state: &mut StreamState, chunk: &ChatChunk) -> Vec<String> {
    let mut events = Vec::new();

    let Some(choice) = chunk.choices.first() else {
        return events;
    };

    // Tool call deltas
    if let Some(ref tool_calls) = choice.delta.tool_calls {
        for tc in tool_calls {
            let exists = state.current_tool_calls.contains_key(&tc.index);
            if !exists {
                let id = gen_id("fc");
                state.current_tool_calls.insert(
                    tc.index,
                    StreamToolCall {
                        call_id: tc.id.clone().unwrap_or_else(|| id.clone()),
                        id,
                        name: String::new(),
                        arguments: String::new(),
                        output_index: None,
                        added: false,
                        done: false,
                    },
                );
            }

            let mut arguments_delta = String::new();
            let needs_output_index = state
                .current_tool_calls
                .get(&tc.index)
                .and_then(|entry| entry.output_index)
                .is_none();
            let allocated_output_index = if needs_output_index {
                Some(state.allocate_output_index())
            } else {
                None
            };

            {
                let entry = state.current_tool_calls.get_mut(&tc.index).unwrap();

                if let Some(output_index) = allocated_output_index {
                    entry.output_index = Some(output_index);
                }
                if let Some(ref id) = tc.id {
                    entry.call_id = id.clone();
                }
                if let Some(ref func) = tc.function {
                    if let Some(ref name) = func.name {
                        entry.name.push_str(name);
                    }
                    if let Some(ref args) = func.arguments {
                        arguments_delta.push_str(args);
                        entry.arguments.push_str(args);
                    }
                }

                if !entry.added {
                    let output_index = entry.output_index.unwrap();
                    events.push(format_sse(
                        "response.output_item.added",
                        &serde_json::json!({
                            "type": "response.output_item.added",
                            "output_index": output_index,
                            "item": {
                                "id": entry.id,
                                "type": "function_call",
                                "call_id": entry.call_id,
                                "name": entry.name,
                                "arguments": "",
                                "status": "in_progress"
                            }
                        }),
                    ));
                    entry.added = true;
                }

                if !arguments_delta.is_empty() {
                    events.push(format_sse(
                        "response.function_call_arguments.delta",
                        &serde_json::json!({
                            "type": "response.function_call_arguments.delta",
                            "output_index": entry.output_index.unwrap(),
                            "item_id": entry.id,
                            "delta": arguments_delta
                        }),
                    ));
                }
            }
        }
    }

    // Text content deltas
    if let Some(ref content) = choice.delta.content {
        if !state.content_started {
            // emit output_item.added for message
            let output_index = state.allocate_output_index();
            state.message_output_index = Some(output_index);
            events.push(format_sse(
                "response.output_item.added",
                &serde_json::json!({
                    "type": "response.output_item.added",
                    "output_index": output_index,
                    "item": {
                        "id": state.message_item_id,
                        "type": "message",
                        "role": "assistant",
                        "status": "in_progress",
                        "content": []
                    }
                }),
            ));

            // emit content_part.added
            events.push(format_sse(
                "response.content_part.added",
                &serde_json::json!({
                    "type": "response.content_part.added",
                    "output_index": output_index,
                    "content_index": 0,
                    "part": {
                        "type": "output_text",
                        "text": "",
                        "annotations": []
                    }
                }),
            ));

            state.content_started = true;
        }

        state.accumulated_text.push_str(content);

        events.push(format_sse(
            "response.output_text.delta",
            &serde_json::json!({
                "type": "response.output_text.delta",
                "output_index": state.message_output_index.unwrap_or(0),
                "content_index": 0,
                "delta": content
            }),
        ));
    }

    // finish_reason: stop
    if choice.finish_reason.is_some() && choice.finish_reason.as_deref() != Some("tool_calls") {
        // close content part
        complete_message(state, &mut events);
    }

    // finish_reason: tool_calls — emit function_call items
    if choice.finish_reason.as_deref() == Some("tool_calls") {
        // close text portion if any
        complete_message(state, &mut events);

        // emit each tool call as function_call output item
        let tool_call_indexes: Vec<u32> = state.current_tool_calls.keys().copied().collect();
        for tool_call_index in tool_call_indexes {
            let needs_output_index = state
                .current_tool_calls
                .get(&tool_call_index)
                .and_then(|entry| entry.output_index)
                .is_none();
            let allocated_output_index = if needs_output_index {
                Some(state.allocate_output_index())
            } else {
                None
            };
            let tc = state.current_tool_calls.get_mut(&tool_call_index).unwrap();

            if let Some(output_index) = allocated_output_index {
                tc.output_index = Some(output_index);
            }
            let output_index = tc.output_index.unwrap();

            if !tc.added {
                events.push(format_sse(
                    "response.output_item.added",
                    &serde_json::json!({
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": {
                            "id": tc.id,
                            "type": "function_call",
                            "call_id": tc.call_id,
                            "name": tc.name,
                            "arguments": "",
                            "status": "in_progress"
                        }
                    }),
                ));
                tc.added = true;
            }

            events.push(format_sse(
                "response.function_call_arguments.done",
                &serde_json::json!({
                    "type": "response.function_call_arguments.done",
                    "output_index": output_index,
                    "item_id": tc.id,
                    "arguments": tc.arguments
                }),
            ));

            let item = serde_json::json!({
                "id": tc.id,
                "type": "function_call",
                "call_id": tc.call_id,
                "name": tc.name,
                "arguments": tc.arguments,
                "status": "completed"
            });

            events.push(format_sse(
                "response.output_item.done",
                &serde_json::json!({
                    "type": "response.output_item.done",
                    "output_index": output_index,
                    "item": item
                }),
            ));
            state.output_items.insert(output_index, item);
            tc.done = true;
        }
    }

    events
}

fn complete_message(state: &mut StreamState, events: &mut Vec<String>) {
    if !state.content_started || state.message_done {
        return;
    }

    let output_index = state.message_output_index.unwrap_or(0);

    events.push(format_sse(
        "response.output_text.done",
        &serde_json::json!({
            "type": "response.output_text.done",
            "output_index": output_index,
            "content_index": 0,
            "text": state.accumulated_text
        }),
    ));

    events.push(format_sse(
        "response.content_part.done",
        &serde_json::json!({
            "type": "response.content_part.done",
            "output_index": output_index,
            "content_index": 0,
            "part": {
                "type": "output_text",
                "text": state.accumulated_text,
                "annotations": []
            }
        }),
    ));

    let item = serde_json::json!({
        "id": state.message_item_id,
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{
            "type": "output_text",
            "text": state.accumulated_text,
            "annotations": []
        }]
    });

    events.push(format_sse(
        "response.output_item.done",
        &serde_json::json!({
            "type": "response.output_item.done",
            "output_index": output_index,
            "item": item
        }),
    ));

    state.output_items.insert(output_index, item);
    state.message_done = true;
}

pub fn convert_stream_finish(state: &mut StreamState) -> Vec<String> {
    let mut events = Vec::new();
    complete_message(state, &mut events);
    events
}

pub fn convert_stream_end(state: &StreamState, usage: Option<&ChatUsage>) -> String {
    let usage_val = match usage {
        Some(u) => serde_json::json!({
            "input_tokens": u.prompt_tokens,
            "output_tokens": u.completion_tokens,
            "total_tokens": u.total_tokens
        }),
        None => serde_json::json!({
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0
        }),
    };

    format_sse(
        "response.completed",
        &serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": state.response_id,
                "object": "response",
                "created_at": now_secs(),
                "model": state.model,
                "status": "completed",
                "output": state.output_items.values().cloned().collect::<Vec<_>>(),
                "usage": usage_val
            }
        }),
    )
}

pub struct SseDecoder {
    buffer: Vec<u8>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self { buffer: Vec::new() }
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();

        while let Some((index, delimiter_len)) = next_sse_boundary(&self.buffer) {
            let raw = self.buffer[..index].to_vec();
            self.buffer.drain(..index + delimiter_len);

            if let Some(data) = parse_sse_data(&raw) {
                events.push(data);
            }
        }

        events
    }
}

fn next_sse_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = find_bytes(buffer, b"\n\n").map(|index| (index, 2));
    let crlf = find_bytes(buffer, b"\r\n\r\n").map(|index| (index, 4));

    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 < right.0 { left } else { right }),
        (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
        (None, None) => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_sse_data(raw: &[u8]) -> Option<String> {
    let raw = std::str::from_utf8(raw).ok()?;
    let data_lines: Vec<String> = raw
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let data = line.strip_prefix("data:")?;
            Some(data.strip_prefix(' ').unwrap_or(data).to_string())
        })
        .collect();

    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

fn format_sse(event: &str, data: &serde_json::Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn responses_to_chat_should_preserve_full_context_and_tool_calls() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "test-model",
            "instructions": "system prompt",
            "input": [
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hello"}]
                },
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "hi"}]
                },
                {
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": "{\"q\":\"rust\"}"
                },
                {
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "{\"result\":\"ok\"}"
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "continue"}]
                }
            ],
            "tools": [{
                "type": "function",
                "name": "lookup",
                "description": "Lookup",
                "parameters": {"type": "object"}
            }],
            "parallel_tool_calls": true
        }))
        .unwrap();

        let chat = responses_to_chat(&req);

        assert_eq!(chat.messages.len(), 6);
        assert_eq!(chat.messages[0].role, "system");
        assert_eq!(chat.messages[1].content, Some(json!("hello")));
        assert_eq!(chat.messages[2].content, Some(json!("hi")));
        assert_eq!(
            chat.messages[3].tool_calls.as_ref().unwrap()[0].id,
            "call_1"
        );
        assert_eq!(chat.messages[4].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(chat.messages[5].content, Some(json!("continue")));
        assert_eq!(chat.tools.as_ref().unwrap()[0].function.name, "lookup");
    }

    #[test]
    fn responses_to_chat_should_convert_responses_function_tool_choice() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "test-model",
            "input": "hello",
            "tool_choice": {
                "type": "function",
                "name": "lookup"
            }
        }))
        .unwrap();

        let chat = responses_to_chat(&req);

        assert_eq!(
            chat.tool_choice,
            Some(json!({
                "type": "function",
                "function": {
                    "name": "lookup"
                }
            }))
        );
    }

    #[test]
    fn streaming_tool_calls_should_emit_argument_deltas_and_complete_output() {
        let mut state = StreamState::new();
        let first = ChatChunk {
            id: "chunk_1".into(),
            model: "test-model".into(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatChunkDelta {
                    role: Some("assistant".into()),
                    content: None,
                    tool_calls: Some(vec![ChatChunkToolCall {
                        index: 0,
                        id: Some("call_1".into()),
                        call_type: Some("function".into()),
                        function: Some(ChatChunkFunction {
                            name: Some("lookup".into()),
                            arguments: Some("{\"q\"".into()),
                        }),
                    }]),
                },
                finish_reason: None,
            }],
            usage: None,
        };

        let events = convert_stream_chunk(&mut state, &first).join("");

        assert!(events.contains("response.output_item.added"));
        assert!(events.contains("response.function_call_arguments.delta"));

        let second = ChatChunk {
            id: "chunk_2".into(),
            model: "test-model".into(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatChunkDelta {
                    role: None,
                    content: None,
                    tool_calls: Some(vec![ChatChunkToolCall {
                        index: 0,
                        id: None,
                        call_type: None,
                        function: Some(ChatChunkFunction {
                            name: None,
                            arguments: Some(":\"rust\"}".into()),
                        }),
                    }]),
                },
                finish_reason: Some("tool_calls".into()),
            }],
            usage: None,
        };

        let events = convert_stream_chunk(&mut state, &second).join("");
        let end = convert_stream_end(&state, None);

        assert!(events.contains("response.function_call_arguments.done"));
        assert!(events.contains("\"arguments\":\"{\\\"q\\\":\\\"rust\\\"}\""));
        assert!(end.contains("\"type\":\"function_call\""));
    }

    #[test]
    fn stream_finish_should_complete_open_text_output() {
        let mut state = StreamState::new();
        convert_stream_start(&mut state, "test-model");
        let chunk = ChatChunk {
            id: "chunk_1".into(),
            model: "test-model".into(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatChunkDelta {
                    role: Some("assistant".into()),
                    content: Some("hello".into()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
            usage: None,
        };

        convert_stream_chunk(&mut state, &chunk);
        let finish = convert_stream_finish(&mut state).join("");
        let end = convert_stream_end(&state, None);

        assert!(finish.contains("response.output_item.done"));
        assert!(end.contains("\"model\":\"test-model\""));
        assert!(end.contains("\"text\":\"hello\""));
    }

    #[test]
    fn sse_decoder_should_preserve_multibyte_content_split_across_chunks() {
        let mut decoder = SseDecoder::new();
        let event = "data: {\"text\":\"你好\"}\n\n";
        let split = event.find("好").unwrap() + 1;

        assert!(decoder.push(&event.as_bytes()[..split]).is_empty());
        let events = decoder.push(&event.as_bytes()[split..]);

        assert_eq!(events, vec!["{\"text\":\"你好\"}".to_string()]);
    }
}
