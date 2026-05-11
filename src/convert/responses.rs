use super::{gen_id, now_secs};
use crate::types::*;

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
                reasoning_content: choice.message.reasoning_content.clone(),
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

pub fn anthropic_to_responses(resp: &AnthropicResponse) -> ResponsesResponse {
    let mut output = Vec::new();

    for block in &resp.content {
        match block.block_type.as_str() {
            "text" => {
                if let Some(text) = block.text.as_ref()
                    && !text.is_empty()
                {
                    output.push(ResponseOutputItem::Message {
                        id: gen_id("msg"),
                        status: "completed".into(),
                        role: resp.role.clone(),
                        reasoning_content: None,
                        content: vec![ResponseContentPart {
                            part_type: "output_text".into(),
                            text: text.clone(),
                            annotations: vec![],
                        }],
                    });
                }
            }
            "tool_use" => output.push(ResponseOutputItem::FunctionCall {
                id: gen_id("fc"),
                status: "completed".into(),
                call_id: block.id.clone().unwrap_or_default(),
                name: block.name.clone().unwrap_or_default(),
                arguments: block
                    .input
                    .as_ref()
                    .map(|input| input.to_string())
                    .unwrap_or_else(|| "{}".into()),
            }),
            _ => {}
        }
    }

    let usage = resp
        .usage
        .as_ref()
        .map(anthropic_usage_to_responses_usage)
        .unwrap_or(ResponsesUsage {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
        });

    ResponsesResponse {
        id: gen_id("resp"),
        object: "response".into(),
        created_at: now_secs(),
        model: resp.model.clone(),
        status: "completed".into(),
        output,
        usage,
    }
}

pub fn anthropic_usage_to_chat_usage(usage: &AnthropicUsage) -> ChatUsage {
    ChatUsage {
        prompt_tokens: usage.input_tokens,
        completion_tokens: usage.output_tokens,
        total_tokens: usage.input_tokens + usage.output_tokens,
    }
}

fn anthropic_usage_to_responses_usage(usage: &AnthropicUsage) -> ResponsesUsage {
    ResponsesUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        total_tokens: usage.input_tokens + usage.output_tokens,
    }
}
