use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── Responses API ───

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponsesInput {
    String(String),
    Items(Vec<ResponsesInputItem>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesInputItem {
    #[serde(default)]
    #[serde(rename = "type")]
    pub item_type: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub call_id: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponsesRequest {
    pub model: String,
    pub input: ResponsesInput,
    #[serde(default)]
    pub instructions: Option<String>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub temperature: Option<f64>,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
    #[serde(default)]
    pub top_p: Option<f64>,
    #[serde(default)]
    pub tools: Option<Vec<ResponsesTool>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponsesUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponseContentPart {
    #[serde(rename = "type")]
    pub part_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub annotations: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ResponseOutputItem {
    #[serde(rename = "message")]
    Message {
        id: String,
        status: String,
        role: String,
        content: Vec<ResponseContentPart>,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        status: String,
        call_id: String,
        name: String,
        arguments: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct ResponsesResponse {
    pub id: String,
    pub object: String,
    pub created_at: u64,
    pub model: String,
    pub status: String,
    pub output: Vec<ResponseOutputItem>,
    pub usage: ResponsesUsage,
}

// ─── ChatCompletions API ───

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: ChatFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatFunction {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatTool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ChatToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatToolFunction {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ChatTool>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatResponseMessage,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponseMessage {
    pub role: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChunk {
    pub id: String,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
    #[serde(default)]
    pub usage: Option<ChatUsage>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatChunkDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChunkDelta {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatChunkToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct ChatChunkToolCall {
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default, rename = "type")]
    pub call_type: Option<String>,
    #[serde(default)]
    pub function: Option<ChatChunkFunction>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatChunkFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}
