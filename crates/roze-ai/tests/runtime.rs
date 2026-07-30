use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use futures_util::StreamExt;
use roze_ai::{
    AgentOptions, AiError, AiEvent, AiRuntime, ChatModel, ContentBlock, GenerationOptions, Message,
    MessageRole, MockChatModel, ModelRequest, ModelResponse, ModelUsage, Tool, ToolCall,
    ToolDefinition,
};
use roze_config::{AiConfig, AiProviderConfig, AiProviderKind};
use roze_context::Context;
use roze_error::RozeError;
use serde_json::{json, Value};

struct SumTool;

#[async_trait]
impl Tool for SumTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition::new(
            "sum",
            "Adds two integers",
            json!({
                "type": "object",
                "properties": {
                    "left": {"type": "integer"},
                    "right": {"type": "integer"}
                },
                "required": ["left", "right"]
            }),
        )
        .with_permissions(["calculator:use"])
    }

    async fn invoke(&self, _context: &Context, arguments: Value) -> Result<Value, AiError> {
        let left = arguments
            .get("left")
            .and_then(Value::as_i64)
            .ok_or_else(|| AiError::InvalidRequest("left must be an integer".to_string()))?;
        let right = arguments
            .get("right")
            .and_then(Value::as_i64)
            .ok_or_else(|| AiError::InvalidRequest("right must be an integer".to_string()))?;
        Ok(json!({"value": left + right}))
    }
}

#[tokio::test]
async fn runtime_returns_a_direct_model_answer() {
    let model = MockChatModel::new([ModelResponse::text("hello")]);
    let runtime = AiRuntime::new("mock", Arc::new(model)).unwrap();
    let output = runtime
        .invoke(&Context::background(), [Message::user("hi")])
        .await
        .unwrap();

    assert_eq!(output.message, Message::assistant("hello"));
    assert_eq!(output.steps, 1);
    assert!(matches!(
        output.events.last(),
        Some(AiEvent::RunCompleted { steps: 1, .. })
    ));
}

#[tokio::test]
async fn model_default_stream_preserves_roze_request_context() {
    let model = MockChatModel::new([ModelResponse::text("hello")]);
    let context = Context::background_with_request_id_and_trace_id("request-ai-1", "trace-ai-1");
    let mut stream = model.stream(
        &context,
        ModelRequest {
            messages: vec![Message::user("hi")],
            tools: Vec::new(),
            options: GenerationOptions::default(),
        },
    );

    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(
        event,
        AiEvent::ModelCompleted {
            run_id,
            step: 1,
            ..
        } if run_id == "request-ai-1"
    ));
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn runtime_executes_a_permission_checked_tool_loop() {
    let call = ToolCall::new("call-1", "sum", json!({"left": 2, "right": 3}));
    let first = ModelResponse::tool_calls([call.clone()]);
    let mut second = ModelResponse::text("5");
    second.usage = ModelUsage {
        input_tokens: 4,
        output_tokens: 1,
    };
    let model = MockChatModel::new([first, second]);
    let mut runtime = AiRuntime::new("mock", Arc::new(model)).unwrap();
    runtime.register_tool(SumTool).unwrap();

    let context = Context::background().with_permissions(["calculator:use"]);
    let output = runtime
        .invoke(&context, [Message::user("2 + 3")])
        .await
        .unwrap();

    assert_eq!(output.message, Message::assistant("5"));
    assert_eq!(output.steps, 2);
    assert_eq!(output.usage.output_tokens, 1);
    assert!(output.history.iter().any(|message| {
        message.role == MessageRole::Tool
            && matches!(
                message.content.as_slice(),
                [ContentBlock::ToolResult { output, .. }]
                    if output == &json!({"value": 5})
            )
    }));
}

#[tokio::test]
async fn runtime_fails_closed_when_tool_permission_is_missing() {
    let call = ToolCall::new("call-1", "sum", json!({"left": 2, "right": 3}));
    let model = MockChatModel::new([ModelResponse::tool_calls([call])]);
    let mut runtime = AiRuntime::new("mock", Arc::new(model)).unwrap();
    runtime.register_tool(SumTool).unwrap();

    let error = runtime
        .invoke(&Context::background(), [Message::user("2 + 3")])
        .await
        .unwrap_err();

    assert_eq!(error, AiError::PermissionDenied("sum".to_string()));
    assert_eq!(RozeError::from(error), RozeError::Forbidden);
}

#[tokio::test]
async fn runtime_honors_context_cancellation_and_step_bounds() {
    let cancelled = Context::background();
    cancelled.cancel();
    let runtime = AiRuntime::new(
        "mock",
        Arc::new(MockChatModel::new([ModelResponse::text("unused")])),
    )
    .unwrap();
    assert_eq!(
        runtime
            .invoke(&cancelled, [Message::user("hi")])
            .await
            .unwrap_err(),
        AiError::Cancelled
    );

    let call = ToolCall::new("call-1", "sum", json!({"left": 1, "right": 1}));
    let model = MockChatModel::new([ModelResponse::tool_calls([call])]);
    let mut runtime = AiRuntime::new("mock", Arc::new(model)).unwrap();
    runtime.register_tool(SumTool).unwrap();
    let agent = runtime
        .agent(
            None,
            AgentOptions {
                max_steps: 1,
                ..AgentOptions::default()
            },
        )
        .unwrap();

    let error = agent
        .invoke(
            &Context::background().with_permissions(["calculator:use"]),
            [Message::user("1 + 1")],
        )
        .await
        .unwrap_err();
    assert_eq!(error, AiError::MaxStepsExceeded { max_steps: 1 });
}

#[test]
fn runtime_rejects_duplicate_models_and_tools() {
    let mut runtime =
        AiRuntime::new("mock", Arc::new(MockChatModel::default())).expect("valid runtime");
    assert!(runtime
        .register_model("mock", Arc::new(MockChatModel::default()))
        .is_err());
    runtime.register_tool(SumTool).unwrap();
    assert!(runtime.register_tool(SumTool).is_err());
}

#[test]
fn runtime_builds_all_configured_providers_and_agent_defaults() {
    let provider = |model: &str| AiProviderConfig {
        kind: AiProviderKind::OpenaiCompatible,
        base_url: "https://api.example.com/v1".to_string(),
        api_key: None,
        model: model.to_string(),
        timeout_ms: 5_000,
    };
    let runtime = AiRuntime::from_config(&AiConfig {
        default_provider: "primary".to_string(),
        max_steps: 11,
        providers: BTreeMap::from([
            ("primary".to_string(), provider("primary-model")),
            ("fast".to_string(), provider("fast-model")),
        ]),
    })
    .unwrap();

    assert_eq!(runtime.default_model(), "primary");
    assert_eq!(runtime.model_count(), 2);
    assert_eq!(runtime.default_agent_options().max_steps, 11);
}
