use super::*;

const MAX_THINKING_BUDGET: u64 = 32_000;
const MAX_TOKENS_VALUE: u64 = 64_000;
const MIN_MAX_TOKENS_FOR_BUDGET: u64 = MAX_THINKING_BUDGET + 1;

pub(super) fn rectify_anthropic_retry_body(
    config: &crate::config::RectifierConfig,
    mut request: AnthropicRequest,
    error_bytes: &[u8],
) -> Option<Vec<u8>> {
    if !config.enabled {
        return None;
    }

    let error_message = extract_error_message(error_bytes);
    let mut applied = false;

    if config.thinking_signature && should_rectify_thinking_signature(error_message.as_deref()) {
        applied |= rectify_thinking_signature(&mut request);
    }

    if config.thinking_budget && should_rectify_thinking_budget(error_message.as_deref()) {
        applied |= rectify_thinking_budget(&mut request);
    }

    if applied {
        serde_json::to_vec(&request).ok()
    } else {
        None
    }
}

fn extract_error_message(error_bytes: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(error_bytes).ok()?;
    value
        .pointer("/error/message")
        .or_else(|| value.pointer("/message"))
        .and_then(|message| message.as_str())
        .map(ToString::to_string)
        .or_else(|| Some(value.to_string()))
}

fn should_rectify_thinking_signature(error_message: Option<&str>) -> bool {
    let Some(message) = error_message else {
        return false;
    };
    let lower = message.to_ascii_lowercase();

    (lower.contains("invalid")
        && lower.contains("signature")
        && lower.contains("thinking")
        && lower.contains("block"))
        || (lower.contains("thought signature")
            && (lower.contains("not valid") || lower.contains("invalid")))
        || lower.contains("must start with a thinking block")
        || (lower.contains("expected")
            && (lower.contains("thinking") || lower.contains("redacted_thinking"))
            && lower.contains("found")
            && lower.contains("tool_use"))
        || (lower.contains("signature") && lower.contains("field required"))
        || (lower.contains("signature") && lower.contains("extra inputs are not permitted"))
        || ((lower.contains("thinking") || lower.contains("redacted_thinking"))
            && lower.contains("cannot be modified"))
        || lower.contains("invalid request")
        || lower.contains("illegal request")
        || lower.contains("非法请求")
}

fn should_rectify_thinking_budget(error_message: Option<&str>) -> bool {
    let Some(message) = error_message else {
        return false;
    };
    let lower = message.to_ascii_lowercase();
    let has_budget = lower.contains("budget_tokens") || lower.contains("budget tokens");
    let has_thinking = lower.contains("thinking");
    let has_minimum = lower.contains("greater than or equal to 1024")
        || lower.contains(">= 1024")
        || (lower.contains("1024") && lower.contains("input should be"));

    has_budget && has_thinking && has_minimum
}

fn rectify_thinking_signature(request: &mut AnthropicRequest) -> bool {
    let mut applied = false;

    for message in &mut request.messages {
        let serde_json::Value::Array(content) = &mut message.content else {
            continue;
        };

        let mut next = Vec::with_capacity(content.len());
        for mut block in std::mem::take(content) {
            match block.get("type").and_then(|value| value.as_str()) {
                Some("thinking") | Some("redacted_thinking") => {
                    applied = true;
                    continue;
                }
                _ => {}
            }

            if let Some(obj) = block.as_object_mut()
                && obj.remove("signature").is_some()
            {
                applied = true;
            }
            next.push(block);
        }
        *content = next;
    }

    if should_remove_top_level_thinking(request) {
        request.extra.remove("thinking");
        applied = true;
    }

    applied
}

fn should_remove_top_level_thinking(request: &AnthropicRequest) -> bool {
    let thinking_enabled = request
        .extra
        .get("thinking")
        .and_then(|thinking| thinking.get("type"))
        .and_then(|value| value.as_str())
        == Some("enabled");
    if !thinking_enabled {
        return false;
    }

    let Some(last_assistant) = request
        .messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
    else {
        return false;
    };
    let Some(content) = last_assistant.content.as_array() else {
        return false;
    };
    let Some(first) = content.first() else {
        return false;
    };
    let first_type = first.get("type").and_then(|value| value.as_str());
    if first_type == Some("thinking") || first_type == Some("redacted_thinking") {
        return false;
    }

    content
        .iter()
        .any(|block| block.get("type").and_then(|value| value.as_str()) == Some("tool_use"))
}

fn rectify_thinking_budget(request: &mut AnthropicRequest) -> bool {
    let before_max_tokens = request.max_tokens as u64;
    let before_thinking = request.extra.get("thinking").cloned();

    if before_thinking
        .as_ref()
        .and_then(|thinking| thinking.get("type"))
        .and_then(|value| value.as_str())
        == Some("adaptive")
    {
        return false;
    }

    let thinking = request
        .extra
        .entry("thinking".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !thinking.is_object() {
        *thinking = serde_json::json!({});
    }
    if let Some(obj) = thinking.as_object_mut() {
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("enabled".to_string()),
        );
        obj.insert(
            "budget_tokens".to_string(),
            serde_json::Value::Number(MAX_THINKING_BUDGET.into()),
        );
    }

    if before_max_tokens < MIN_MAX_TOKENS_FOR_BUDGET {
        request.max_tokens = MAX_TOKENS_VALUE as u32;
    }

    before_max_tokens != request.max_tokens as u64
        || before_thinking != request.extra.get("thinking").cloned()
}
