use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The author of a provider-neutral chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A model-requested tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
}

impl ToolCall {
    pub fn new(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }
}

/// Provider-neutral message content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    ToolCall {
        call: ToolCall,
    },
    ToolResult {
        tool_call_id: String,
        name: String,
        output: Value,
        #[serde(default)]
        is_error: bool,
    },
}

/// A message exchanged between an application, model, and tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn new(role: MessageRole, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    pub fn text(role: MessageRole, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentBlock::Text { text: text.into() }])
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::text(MessageRole::System, text)
    }

    pub fn user(text: impl Into<String>) -> Self {
        Self::text(MessageRole::User, text)
    }

    pub fn assistant(text: impl Into<String>) -> Self {
        Self::text(MessageRole::Assistant, text)
    }

    pub fn assistant_tool_calls(calls: impl IntoIterator<Item = ToolCall>) -> Self {
        Self::new(
            MessageRole::Assistant,
            calls
                .into_iter()
                .map(|call| ContentBlock::ToolCall { call })
                .collect(),
        )
    }

    pub fn tool_result(call: &ToolCall, output: Value) -> Self {
        Self::new(
            MessageRole::Tool,
            vec![ContentBlock::ToolResult {
                tool_call_id: call.id.clone(),
                name: call.name.clone(),
                output,
                is_error: false,
            }],
        )
    }

    pub fn tool_calls(&self) -> impl Iterator<Item = &ToolCall> {
        self.content.iter().filter_map(|block| match block {
            ContentBlock::ToolCall { call } => Some(call),
            ContentBlock::Text { .. } | ContentBlock::ToolResult { .. } => None,
        })
    }
}

/// Whether a tool is read-only or may change external state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolEffect {
    #[default]
    ReadOnly,
    Mutation,
    ExternalSideEffect,
}

/// Provider-neutral tool metadata exposed to a model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_permissions: Vec<String>,
    #[serde(default)]
    pub effect: ToolEffect,
}

impl ToolDefinition {
    pub fn new(name: impl Into<String>, description: impl Into<String>, parameters: Value) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            required_permissions: Vec::new(),
            effect: ToolEffect::ReadOnly,
        }
    }

    pub fn with_permissions(
        mut self,
        permissions: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.required_permissions = permissions.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_effect(mut self, effect: ToolEffect) -> Self {
        self.effect = effect;
        self
    }
}

/// Common generation controls. Providers may ignore unsupported options.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GenerationOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop: Vec<String>,
    pub response_format: Option<Value>,
}

/// A provider-neutral model request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
    #[serde(default)]
    pub options: GenerationOptions,
}

/// Token accounting returned by a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl ModelUsage {
    pub(crate) fn add_assign(&mut self, other: Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
    }
}

/// Why a model stopped producing output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    #[default]
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Other,
}

/// A complete provider-neutral model response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub message: Message,
    #[serde(default)]
    pub usage: ModelUsage,
    #[serde(default)]
    pub finish_reason: FinishReason,
}

impl ModelResponse {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            message: Message::assistant(text),
            usage: ModelUsage::default(),
            finish_reason: FinishReason::Stop,
        }
    }

    pub fn tool_calls(calls: impl IntoIterator<Item = ToolCall>) -> Self {
        Self {
            message: Message::assistant_tool_calls(calls),
            usage: ModelUsage::default(),
            finish_reason: FinishReason::ToolCalls,
        }
    }
}

/// Events emitted by a model or agent run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AiEvent {
    RunStarted {
        run_id: String,
        model: String,
    },
    MessageDelta {
        run_id: String,
        step: usize,
        delta: String,
    },
    ModelCompleted {
        run_id: String,
        step: usize,
        response: ModelResponse,
    },
    ToolStarted {
        run_id: String,
        step: usize,
        call: ToolCall,
    },
    ToolCompleted {
        run_id: String,
        step: usize,
        call: ToolCall,
        output: Value,
    },
    RunCompleted {
        run_id: String,
        steps: usize,
        usage: ModelUsage,
    },
}
