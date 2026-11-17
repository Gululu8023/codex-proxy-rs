//! 提取原生协议文本及来源，不改写 thinking、签名、图片或请求本身

use gateway_core::operation::Operation;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    CurrentUser,
    History,
    System,
    Instructions,
    Developer,
    Extension,
    ToolArguments,
    ToolOutput,
}
impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::CurrentUser => "current_user",
            Self::History => "history",
            Self::System => "system",
            Self::Instructions => "instructions",
            Self::Developer => "developer",
            Self::Extension => "extension",
            Self::ToolArguments => "tool_arguments",
            Self::ToolOutput => "tool_output",
        }
    }
    pub fn controls_request(self) -> bool {
        matches!(
            self,
            Self::CurrentUser
                | Self::System
                | Self::Instructions
                | Self::Developer
                | Self::Extension
        )
    }
}
#[derive(Clone, Serialize)]
pub struct Segment {
    pub origin: Origin,
    pub text: String,
    pub source: String,
    pub turn: Option<usize>,
    pub representation: &'static str,
    pub root: usize,
}
#[derive(Default, Serialize)]
pub struct Envelope {
    pub segments: Vec<Segment>,
    pub unclassified: bool,
    pub text_only: bool,
    #[serde(skip)]
    source: String,
    #[serde(skip)]
    turn: Option<usize>,
}
impl Envelope {
    pub fn from_text(text: String, origin: Origin) -> Self {
        let mut envelope = Self {
            source: "evaluation".into(),
            ..Self::default()
        };
        envelope.push(&text, origin);
        envelope
    }
    pub fn extract(operation: &Operation) -> Result<Self, ()> {
        let body = operation.wire_body().map_err(|_| ())?;
        let value: Value = serde_json::from_slice(&body).map_err(|_| ())?;
        if !value.is_object() {
            return Err(());
        }
        let mut envelope = Self::default();
        for (field, origin) in [
            ("instructions", Origin::Instructions),
            ("system", Origin::System),
            ("prompt", Origin::CurrentUser),
            ("query", Origin::CurrentUser),
        ] {
            envelope.source = field.into();
            if let Some(value) = value.get(field) {
                envelope.content(value, origin, 0);
            }
        }
        if let Some(tools) = value.get("tools") {
            envelope.source = "tools".into();
            envelope.strings(tools, Origin::System, 0);
        }
        for field in ["output_config", "response_format", "text"] {
            if let Some(schema) = value.get(field) {
                envelope.source = field.into();
                envelope.strings(schema, Origin::System, 0);
            }
        }
        for field in ["input", "messages"] {
            if let Some(items) = value.get(field).and_then(Value::as_array) {
                // 工具回传不创建新的用户意图，继续关联最近一条实际用户文本。
                let current = items.iter().rposition(|value| {
                    value["role"] == "user" && user_instruction(&value["content"])
                });
                for (index, item) in items.iter().enumerate() {
                    envelope.source = format!("{field}[{index}]");
                    envelope.turn = Some(index);
                    let origin = match item["role"].as_str() {
                        Some("user") if Some(index) == current => Origin::CurrentUser,
                        Some("system") => Origin::System,
                        Some("developer") => Origin::Developer,
                        _ => Origin::History,
                    };
                    envelope.content(item, origin, 0);
                }
            } else if let Some(value) = value.get(field) {
                envelope.source = field.into();
                envelope.turn = None;
                envelope.content(value, Origin::CurrentUser, 0);
            }
        }
        // 未知扩展字段仍完整提取文本，不能因字段未列入协议白名单而漏审
        envelope.turn = None;
        envelope.source = "extensions".into();
        for (field, content) in value.as_object().expect("object") {
            // Codex 的缓存键、返回字段选择和客户端元数据是控制字段，不是扩展提示词。
            if !matches!(
                field.as_str(),
                "instructions"
                    | "system"
                    | "prompt"
                    | "query"
                    | "tools"
                    | "input"
                    | "messages"
                    | "model"
                    | "stream"
                    | "max_tokens"
                    | "temperature"
                    | "top_p"
                    | "top_k"
                    | "metadata"
                    | "prompt_cache_key"
                    | "include"
                    | "client_metadata"
                    | "thinking"
                    | "tool_choice"
                    | "stop_sequences"
                    | "service_tier"
                    | "container"
                    | "context_management"
                    | "input_transformations"
                    | "previous_response_id"
                    | "store"
                    | "reasoning"
                    | "parallel_tool_calls"
                    | "output_config"
                    | "response_format"
                    | "text"
            ) {
                let before = envelope.segments.len();
                envelope.strings(content, Origin::Extension, 0);
                if envelope.segments.len() != before {
                    envelope.unclassified = true;
                }
            }
        }
        Ok(envelope)
    }
    fn push(&mut self, text: &str, origin: Origin) {
        if text.is_empty() {
            return;
        }
        let root = self.segments.len();
        self.segments.push(Segment {
            origin,
            text: text.into(),
            source: format!("{}.text[{root}]", self.source),
            turn: self.turn,
            representation: "original",
            root,
        });
    }
    // 普通递归沿用文本来源；已识别的工具块才显式切换来源。
    fn strings(&mut self, value: &Value, origin: Origin, depth: usize) {
        if depth > 64 {
            self.unclassified = true;
            return;
        }
        match value {
            Value::String(text) => self.push(text, origin),
            Value::Array(values) => {
                for value in values {
                    self.strings(value, origin, depth + 1);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.strings(value, origin, depth + 1);
                }
            }
            _ => {}
        }
    }
    fn content(&mut self, value: &Value, origin: Origin, depth: usize) {
        if depth > 64 {
            self.unclassified = true;
            return;
        }
        match value {
            Value::String(text) => self.push(text, origin),
            Value::Array(values) => {
                for value in values {
                    self.content(value, origin, depth + 1);
                }
            }
            Value::Object(values) => {
                let kind = values
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or("message");
                let handled: &[&str] = match kind {
                    // 图片与加密历史不属于文本审查范围，其余扩展文本仍需提取
                    "image" | "input_image" | "image_url" | "redacted_thinking" | "thinking"
                    | "compaction" | "audio" | "input_audio" => {
                        self.text_only = true;
                        return;
                    }
                    "input_file" => {
                        self.text_only = true;
                        &["file_data", "file_id"]
                    }
                    "document" => {
                        if let Some(source) = values.get("source") {
                            match source["type"].as_str() {
                                Some("text") => {
                                    if let Some(data) = source.get("data") {
                                        self.content(data, origin, depth + 1);
                                    }
                                }
                                Some("content") => {
                                    if let Some(content) = source.get("content") {
                                        self.content(content, origin, depth + 1);
                                    }
                                }
                                Some("base64" | "url") => {
                                    self.text_only = true;
                                }
                                _ => self.unclassified = true,
                            }
                        }
                        &["source"]
                    }
                    "tool_use" | "server_tool_use" => {
                        if let Some(input) = values.get("input") {
                            self.strings(input, Origin::ToolArguments, depth + 1);
                        }
                        &["input"]
                    }
                    "function_call" | "custom_tool_call" => {
                        if let Some(arguments) =
                            values.get("arguments").or_else(|| values.get("input"))
                        {
                            if let Some(text) = arguments.as_str() {
                                if let Ok(parsed) = serde_json::from_str::<Value>(text) {
                                    self.strings(&parsed, Origin::ToolArguments, depth + 1);
                                } else {
                                    self.push(text, Origin::ToolArguments);
                                }
                            } else {
                                self.strings(arguments, Origin::ToolArguments, depth + 1);
                            }
                        }
                        &["arguments", "input"]
                    }
                    "tool_result"
                    | "function_call_output"
                    | "custom_tool_call_output"
                    | "web_search_tool_result"
                    | "web_fetch_tool_result"
                    | "code_execution_tool_result"
                    | "bash_code_execution_tool_result"
                    | "text_editor_code_execution_tool_result" => {
                        if let Some(content) =
                            values.get("content").or_else(|| values.get("output"))
                        {
                            self.content(content, Origin::ToolOutput, depth + 1);
                        }
                        &["content", "output"]
                    }
                    "additional_tools" | "tool_search_output" => {
                        self.strings(value, Origin::System, depth + 1);
                        return;
                    }
                    "image_generation_call" => &["result"],
                    "web_search_call"
                    | "local_shell_call"
                    | "local_shell_call_output"
                    | "shell_call"
                    | "shell_call_output"
                    | "apply_patch_call"
                    | "apply_patch_call_output"
                    | "mcp_call"
                    | "mcp_list_tools"
                    | "mcp_approval_request"
                    | "mcp_approval_response"
                    | "tool_search_call"
                    | "code_interpreter_call" => {
                        self.strings(value, origin, depth + 1);
                        return;
                    }
                    "web_search_result"
                    | "web_fetch_result"
                    | "bash_code_execution_result"
                    | "text_editor_code_execution_view_result"
                    | "text_editor_code_execution_create_result"
                    | "text_editor_code_execution_str_replace_result" => {
                        for (key, content) in values {
                            if key != "encrypted_content" && key != "type" {
                                self.strings(content, Origin::ToolOutput, depth + 1);
                            }
                        }
                        return;
                    }
                    "compaction_trigger" => &[],
                    "message" | "text" | "input_text" | "output_text" | "reasoning"
                    | "summary_text" | "refusal" | "tool_reference" => {
                        for key in ["content", "text", "thinking", "summary", "refusal"] {
                            if let Some(text) = values.get(key) {
                                self.content(text, origin, depth + 1);
                            }
                        }
                        &[
                            "content",
                            "text",
                            "thinking",
                            "summary",
                            "refusal",
                            "signature",
                            "encrypted_content",
                        ]
                    }
                    _ => {
                        self.unclassified = true;
                        self.strings(value, origin, depth + 1);
                        return;
                    }
                };
                // 已知块的未来附加字段也不能漏审；签名和二进制仅在对应块中豁免
                for (key, content) in values {
                    if !matches!(
                        key.as_str(),
                        "type"
                            | "role"
                            | "id"
                            | "tool_use_id"
                            | "call_id"
                            | "name"
                            | "cache_control"
                            | "signature"
                            | "encrypted_content"
                    ) && !handled.contains(&key.as_str())
                    {
                        self.strings(content, origin, depth + 1);
                    }
                }
            }
            _ => self.unclassified = true,
        }
    }
}

fn user_instruction(value: &Value) -> bool {
    match value {
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(blocks) => blocks.iter().any(|block| {
            matches!(
                block["type"].as_str(),
                Some("text" | "input_text" | "document")
            )
        }),
        _ => false,
    }
}
