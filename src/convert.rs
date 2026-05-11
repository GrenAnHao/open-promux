use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

mod anthropic;
mod requests;
mod responses;
mod sse;
mod stream;

pub use anthropic::{
    anthropic_stream_event_is_stop, anthropic_stream_event_model,
    anthropic_stream_event_to_chat_chunk, anthropic_stream_event_usage,
};
pub use requests::{responses_to_anthropic, responses_to_chat};
pub use responses::{anthropic_to_responses, chat_to_responses};
pub use sse::SseDecoder;
pub use stream::{
    StreamState, convert_stream_chunk, convert_stream_end, convert_stream_finish,
    convert_stream_start,
};

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

fn format_sse(event: &str, data: &serde_json::Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
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
    fn responses_to_chat_should_map_text_format_to_response_format() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "test-model",
            "input": "hello",
            "text": {
                "format": {
                    "type": "json_object"
                }
            }
        }))
        .unwrap();

        let chat = responses_to_chat(&req);
        let chat_json = serde_json::to_value(&chat).unwrap();

        assert_eq!(
            chat_json["response_format"],
            json!({
                "type": "json_object"
            })
        );
    }

    #[test]
    fn responses_to_anthropic_should_convert_string_tool_choice_modes() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "claude-test",
            "input": "hello",
            "tool_choice": "required"
        }))
        .unwrap();

        let anthropic = responses_to_anthropic(&req);
        let anthropic_json = serde_json::to_value(&anthropic).unwrap();

        assert_eq!(anthropic_json["tool_choice"], json!({"type": "any"}));
    }

    #[test]
    fn responses_to_chat_should_map_developer_role_to_system_for_chat_compatibility() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "test-model",
            "input": [
                {
                    "type": "message",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "follow these rules"}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "hello"}]
                }
            ]
        }))
        .unwrap();

        let chat = responses_to_chat(&req);

        assert_eq!(chat.messages[0].role, "system");
        assert_eq!(chat.messages[0].content, Some(json!("follow these rules")));
        assert_eq!(chat.messages[1].role, "user");
    }

    #[test]
    fn responses_to_chat_should_preserve_reasoning_content_for_deepseek_replay() {
        let req: ResponsesRequest = serde_json::from_value(json!({
            "model": "deepseek-v4-pro",
            "input": [
                {
                    "type": "message",
                    "role": "assistant",
                    "reasoning_content": "previous reasoning",
                    "content": [{"type": "output_text", "text": "previous answer"}]
                },
                {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "continue"}]
                }
            ]
        }))
        .unwrap();

        let chat = responses_to_chat(&req);
        let chat_json = serde_json::to_value(&chat).unwrap();

        assert_eq!(
            chat_json["messages"][0]["reasoning_content"],
            "previous reasoning"
        );
        assert_eq!(chat_json["messages"][0]["content"], "previous answer");
    }

    #[test]
    fn chat_to_responses_should_preserve_reasoning_content_for_replay() {
        let chat_resp: ChatResponse = serde_json::from_value(json!({
            "id": "chatcmpl_1",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "reasoning_content": "new reasoning",
                    "content": "final answer"
                },
                "finish_reason": "stop"
            }]
        }))
        .unwrap();

        let responses = chat_to_responses(&chat_resp);
        let responses_json = serde_json::to_value(&responses).unwrap();

        assert_eq!(
            responses_json["output"][0]["reasoning_content"],
            "new reasoning"
        );
        assert_eq!(
            responses_json["output"][0]["content"][0]["text"],
            "final answer"
        );
    }

    #[test]
    fn response_stream_should_preserve_reasoning_content_in_completed_output() {
        let mut state = StreamState::new();
        convert_stream_start(&mut state, "deepseek-v4-pro");
        let reasoning_chunk: ChatChunk = serde_json::from_value(json!({
            "id": "chunk_1",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "delta": {
                    "role": "assistant",
                    "reasoning_content": "stream reasoning"
                },
                "finish_reason": null
            }]
        }))
        .unwrap();
        let content_chunk: ChatChunk = serde_json::from_value(json!({
            "id": "chunk_1",
            "model": "deepseek-v4-pro",
            "choices": [{
                "index": 0,
                "delta": {
                    "content": "final answer"
                },
                "finish_reason": "stop"
            }]
        }))
        .unwrap();

        convert_stream_chunk(&mut state, &reasoning_chunk);
        convert_stream_chunk(&mut state, &content_chunk);
        let end = convert_stream_end(&state, None);

        assert!(end.contains("\"reasoning_content\":\"stream reasoning\""));
        assert!(end.contains("\"text\":\"final answer\""));
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
                    reasoning_content: None,
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
                    reasoning_content: None,
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
                    reasoning_content: None,
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
