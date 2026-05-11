use crate::types::*;
use std::collections::HashMap;

const DEFAULT_ANTHROPIC_MAX_TOKENS: u32 = 4096;

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

    let mut out = extra
        .iter()
        .filter(|(key, _)| KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();

    if let Some(format) = extra.get("text").and_then(|text| text.get("format")) {
        out.insert("response_format".into(), format.clone());
    }

    out
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

fn chat_compatible_role(role: &str) -> &str {
    match role {
        "developer" => "system",
        _ => role,
    }
}

// ─── Responses → ChatCompletions ───

pub fn responses_to_chat(req: &ResponsesRequest) -> ChatRequest {
    let mut messages: Vec<ChatMessage> = Vec::new();

    // instructions → system message
    if let Some(ref instructions) = req.instructions {
        messages.push(ChatMessage {
            role: "system".into(),
            content: Some(serde_json::Value::String(instructions.clone())),
            reasoning_content: None,
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
                reasoning_content: None,
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
                            reasoning_content: item.reasoning_content.clone(),
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
                            reasoning_content: None,
                            tool_calls: None,
                            tool_call_id: item.call_id.clone(),
                            name: None,
                        });
                    }
                    _ => {
                        // regular message (user/assistant)
                        let role = chat_compatible_role(item.role.as_deref().unwrap_or("user"));
                        messages.push(ChatMessage {
                            role: role.into(),
                            content: responses_content_to_chat_content(item.content.clone()),
                            reasoning_content: item.reasoning_content.clone(),
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

pub fn responses_to_anthropic(req: &ResponsesRequest) -> AnthropicRequest {
    let chat = responses_to_chat(req);
    let mut system_parts = Vec::new();
    let mut messages = Vec::new();

    for message in chat.messages {
        match message.role.as_str() {
            "system" => {
                if let Some(content) = chat_content_to_text(message.content.as_ref()) {
                    system_parts.push(content);
                }
            }
            "assistant" => messages.push(AnthropicMessage {
                role: "assistant".into(),
                content: assistant_message_to_anthropic_content(&message),
            }),
            "tool" => messages.push(AnthropicMessage {
                role: "user".into(),
                content: serde_json::Value::Array(vec![serde_json::json!({
                    "type": "tool_result",
                    "tool_use_id": message.tool_call_id.unwrap_or_default(),
                    "content": chat_content_to_text(message.content.as_ref()).unwrap_or_default()
                })]),
            }),
            _ => messages.push(AnthropicMessage {
                role: "user".into(),
                content: chat_content_to_anthropic_content(message.content.as_ref()),
            }),
        }
    }

    let tools = req.tools.as_ref().map(|tools| {
        tools
            .iter()
            .filter(|tool| tool.tool_type == "function")
            .map(|tool| AnthropicTool {
                name: tool.name.clone().unwrap_or_default(),
                description: tool.description.clone(),
                input_schema: tool
                    .parameters
                    .clone()
                    .unwrap_or_else(|| serde_json::json!({"type": "object"})),
            })
            .collect()
    });

    AnthropicRequest {
        model: req.model.clone(),
        max_tokens: req
            .max_output_tokens
            .unwrap_or(DEFAULT_ANTHROPIC_MAX_TOKENS),
        messages,
        system: if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        },
        stream: req.stream,
        temperature: req.temperature,
        top_p: req.top_p,
        tools,
        tool_choice: anthropic_compatible_tool_choice(req.tool_choice.as_ref()),
        extra: anthropic_compatible_extra(&req.extra),
    }
}

fn chat_content_to_text(content: Option<&serde_json::Value>) -> Option<String> {
    match content? {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(|text| text.as_str()))
                .collect::<Vec<_>>()
                .join("");
            Some(text)
        }
        other => Some(other.to_string()),
    }
}

fn chat_content_to_anthropic_content(content: Option<&serde_json::Value>) -> serde_json::Value {
    match content {
        Some(serde_json::Value::String(value)) => serde_json::Value::String(value.clone()),
        Some(serde_json::Value::Array(parts)) => serde_json::Value::Array(
            parts
                .iter()
                .map(chat_content_part_to_anthropic_part)
                .collect(),
        ),
        Some(other) => serde_json::Value::String(other.to_string()),
        None => serde_json::Value::String(String::new()),
    }
}

fn assistant_message_to_anthropic_content(message: &ChatMessage) -> serde_json::Value {
    let mut parts = Vec::new();
    append_anthropic_parts_from_chat_content(&mut parts, message.content.as_ref());

    if let Some(tool_calls) = message.tool_calls.as_ref() {
        for tool_call in tool_calls {
            parts.push(serde_json::json!({
                "type": "tool_use",
                "id": tool_call.id,
                "name": tool_call.function.name,
                "input": parse_tool_arguments(&tool_call.function.arguments)
            }));
        }
    }

    if parts.len() == 1
        && parts[0].get("type").and_then(|value| value.as_str()) == Some("text")
        && let Some(text) = parts[0].get("text").and_then(|value| value.as_str())
    {
        return serde_json::Value::String(text.to_string());
    }

    serde_json::Value::Array(parts)
}

fn append_anthropic_parts_from_chat_content(
    parts: &mut Vec<serde_json::Value>,
    content: Option<&serde_json::Value>,
) {
    match content {
        Some(serde_json::Value::String(value)) if !value.is_empty() => {
            parts.push(serde_json::json!({"type": "text", "text": value}));
        }
        Some(serde_json::Value::Array(values)) => {
            parts.extend(values.iter().map(chat_content_part_to_anthropic_part));
        }
        Some(value) => {
            parts.push(serde_json::json!({"type": "text", "text": value.to_string()}));
        }
        None => {}
    }
}

fn chat_content_part_to_anthropic_part(part: &serde_json::Value) -> serde_json::Value {
    match part.get("type").and_then(|value| value.as_str()) {
        Some("text") => serde_json::json!({
            "type": "text",
            "text": part.get("text").cloned().unwrap_or_else(|| serde_json::Value::String(String::new()))
        }),
        Some("image_url") => {
            let url = part
                .get("image_url")
                .and_then(|image_url| image_url.get("url"))
                .and_then(|url| url.as_str())
                .unwrap_or_default();
            serde_json::json!({
                "type": "image",
                "source": {
                    "type": "url",
                    "url": url
                }
            })
        }
        _ => part.clone(),
    }
}

fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value) if value.is_object() => value,
        _ => serde_json::json!({}),
    }
}

fn anthropic_compatible_tool_choice(
    tool_choice: Option<&serde_json::Value>,
) -> Option<serde_json::Value> {
    let value = tool_choice?;
    if let Some(choice) = value.as_str() {
        return match choice {
            "auto" => Some(serde_json::json!({"type": "auto"})),
            "required" => Some(serde_json::json!({"type": "any"})),
            "none" => None,
            _ => Some(value.clone()),
        };
    }

    let choice_type = value.get("type").and_then(|value| value.as_str());

    if choice_type == Some("function")
        && let Some(name) = value.get("name").and_then(|value| value.as_str())
    {
        return Some(serde_json::json!({"type": "tool", "name": name}));
    }

    if value.get("type").and_then(|value| value.as_str()) == Some("function")
        && let Some(name) = value
            .get("function")
            .and_then(|function| function.get("name"))
            .and_then(|value| value.as_str())
    {
        return Some(serde_json::json!({"type": "tool", "name": name}));
    }

    match choice_type {
        Some("auto" | "any" | "tool") => Some(value.clone()),
        Some("required") => Some(serde_json::json!({"type": "any"})),
        Some("none") => None,
        _ => Some(value.clone()),
    }
}

fn anthropic_compatible_extra(
    extra: &HashMap<String, serde_json::Value>,
) -> HashMap<String, serde_json::Value> {
    const KEYS: &[&str] = &["metadata", "stop_sequences", "thinking", "top_k"];
    let mut out = extra
        .iter()
        .filter(|(key, _)| KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<HashMap<_, _>>();

    if let Some(stop) = extra.get("stop") {
        let stop_sequences = match stop {
            serde_json::Value::String(value) => serde_json::json!([value]),
            serde_json::Value::Array(_) => stop.clone(),
            _ => serde_json::Value::Null,
        };
        if !stop_sequences.is_null() {
            out.insert("stop_sequences".into(), stop_sequences);
        }
    }

    out
}
