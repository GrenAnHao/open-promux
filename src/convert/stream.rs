use super::{format_sse, gen_id, now_secs};
use crate::types::*;
use std::collections::BTreeMap;

pub struct StreamState {
    pub response_id: String,
    pub message_item_id: String,
    pub model: String,
    pub started: bool,
    pub content_started: bool,
    pub message_done: bool,
    pub message_output_index: Option<usize>,
    pub accumulated_text: String,
    pub accumulated_reasoning_content: String,
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
            accumulated_reasoning_content: String::new(),
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
    if let Some(ref reasoning_content) = choice.delta.reasoning_content {
        state
            .accumulated_reasoning_content
            .push_str(reasoning_content);
    }

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

    let mut item = serde_json::json!({
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
    if !state.accumulated_reasoning_content.is_empty()
        && let Some(obj) = item.as_object_mut()
    {
        obj.insert(
            "reasoning_content".into(),
            serde_json::Value::String(state.accumulated_reasoning_content.clone()),
        );
    }

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
