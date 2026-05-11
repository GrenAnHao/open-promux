use super::responses::anthropic_usage_to_chat_usage;
use crate::types::*;

pub fn anthropic_stream_event_model(event: &AnthropicStreamEvent) -> Option<&str> {
    if event.event_type == "message_start" {
        event.message.as_ref().map(|message| message.model.as_str())
    } else {
        None
    }
}

pub fn anthropic_stream_event_usage(event: &AnthropicStreamEvent) -> Option<ChatUsage> {
    event
        .usage
        .as_ref()
        .or_else(|| event.delta.as_ref().and_then(|delta| delta.usage.as_ref()))
        .map(anthropic_usage_to_chat_usage)
}

pub fn anthropic_stream_event_is_stop(event: &AnthropicStreamEvent) -> bool {
    event.event_type == "message_stop"
}

pub fn anthropic_stream_event_to_chat_chunk(
    event: &AnthropicStreamEvent,
    model: &str,
) -> Option<ChatChunk> {
    let mut delta = ChatChunkDelta {
        role: None,
        content: None,
        reasoning_content: None,
        tool_calls: None,
    };
    let mut finish_reason = None;

    match event.event_type.as_str() {
        "content_block_start" => {
            let block = event.content_block.as_ref()?;
            if block.block_type != "tool_use" {
                return None;
            }
            delta.tool_calls = Some(vec![ChatChunkToolCall {
                index: event.index.unwrap_or(0),
                id: block.id.clone(),
                call_type: Some("function".into()),
                function: Some(ChatChunkFunction {
                    name: block.name.clone(),
                    arguments: None,
                }),
            }]);
        }
        "content_block_delta" => {
            let event_delta = event.delta.as_ref()?;
            if let Some(text) = event_delta.text.as_ref() {
                delta.content = Some(text.clone());
            } else if let Some(partial_json) = event_delta.partial_json.as_ref() {
                delta.tool_calls = Some(vec![ChatChunkToolCall {
                    index: event.index.unwrap_or(0),
                    id: None,
                    call_type: None,
                    function: Some(ChatChunkFunction {
                        name: None,
                        arguments: Some(partial_json.clone()),
                    }),
                }]);
            } else {
                return None;
            }
        }
        "message_delta" => {
            finish_reason = event
                .delta
                .as_ref()
                .and_then(|delta| delta.stop_reason.as_deref())
                .map(|stop_reason| {
                    if stop_reason == "tool_use" {
                        "tool_calls".to_string()
                    } else {
                        "stop".to_string()
                    }
                });
            finish_reason.as_ref()?;
        }
        _ => return None,
    }

    Some(ChatChunk {
        id: String::new(),
        model: model.to_string(),
        choices: vec![ChatChunkChoice {
            index: 0,
            delta,
            finish_reason,
        }],
        usage: None,
    })
}
