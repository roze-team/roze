use std::{collections::BTreeMap, fmt, time::Duration};

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    header::{AUTHORIZATION, RETRY_AFTER},
    StatusCode,
};
use roze_config::AiProviderConfig;
use roze_context::Context;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    tool::check_context, AiError, AiEvent, AiEventStream, ChatModel, ContentBlock, FinishReason,
    Message, MessageRole, ModelRequest, ModelResponse, ModelUsage, ToolCall,
};

/// Chat Completions adapter for OpenAI and compatible `/v1` providers.
#[derive(Clone)]
pub struct OpenAiCompatibleModel {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl OpenAiCompatibleModel {
    pub fn from_config(config: &AiProviderConfig) -> Result<Self, AiError> {
        config
            .validate()
            .map_err(|error| AiError::InvalidRequest(error.to_string()))?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .map_err(|_| AiError::Internal("failed to build AI HTTP client".to_string()))?;
        Ok(Self {
            client,
            base_url: config.base_url.trim_end_matches('/').to_string(),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
        })
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url)
    }

    async fn send(
        &self,
        context: &Context,
        request: ModelRequest,
        stream: bool,
    ) -> Result<reqwest::Response, AiError> {
        check_context(context)?;
        let body = encode_request(&self.model, request, stream)?;
        let mut builder = self
            .client
            .post(self.endpoint())
            .header("x-request-id", context.request_id())
            .header("x-trace-id", context.trace_id())
            .json(&body);
        if let Some(api_key) = self.api_key.as_deref() {
            builder = builder.header(AUTHORIZATION, format!("Bearer {api_key}"));
        }
        if let Some(remaining) = context.remaining_timeout() {
            if remaining.is_zero() {
                return Err(AiError::DeadlineExceeded);
            }
            builder = builder.timeout(remaining);
        }

        let response = builder.send().await.map_err(map_reqwest_error)?;
        check_context(context)?;
        validate_status(&response)?;
        Ok(response)
    }
}

impl fmt::Debug for OpenAiCompatibleModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatibleModel")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl ChatModel for OpenAiCompatibleModel {
    async fn invoke(
        &self,
        context: &Context,
        request: ModelRequest,
    ) -> Result<ModelResponse, AiError> {
        let response = self.send(context, request, false).await?;
        let payload = response
            .json::<ChatCompletionResponse>()
            .await
            .map_err(|_| AiError::Provider("provider returned invalid JSON".to_string()))?;
        decode_response(payload)
    }

    fn stream<'a>(&'a self, context: &'a Context, request: ModelRequest) -> AiEventStream<'a> {
        Box::pin(async_stream::try_stream! {
            let response = self.send(context, request, true).await?;
            let mut chunks = response.bytes_stream();
            let mut buffer = Vec::new();
            let mut text = String::new();
            let mut tool_calls = BTreeMap::<usize, ToolCallAccumulator>::new();
            let mut finish_reason = FinishReason::Stop;
            let mut usage = ModelUsage::default();
            let run_id = context.request_id();

            'response: while let Some(chunk) = chunks.next().await {
                check_context(context)?;
                let chunk = chunk.map_err(map_reqwest_error)?;
                buffer.extend_from_slice(&chunk);
                while let Some(event) = take_sse_event(&mut buffer) {
                    let data = sse_data(&event)?;
                    if data.is_empty() {
                        continue;
                    }
                    if data == "[DONE]" {
                        break 'response;
                    }
                    let chunk = serde_json::from_str::<ChatCompletionChunk>(&data)
                        .map_err(|_| AiError::Provider("provider returned an invalid stream event".to_string()))?;
                    if let Some(error) = chunk.error {
                        return Err(AiError::Provider(format!(
                            "provider stream failed with code {}",
                            error.code.as_deref().unwrap_or("unknown")
                        )))?;
                    }
                    if let Some(chunk_usage) = chunk.usage {
                        usage = chunk_usage.into();
                    }
                    for choice in chunk.choices {
                        if let Some(delta) = choice.delta.content {
                            text.push_str(&delta);
                            yield AiEvent::MessageDelta {
                                run_id: run_id.clone(),
                                step: 1,
                                delta,
                            };
                        }
                        for delta in choice.delta.tool_calls {
                            let call = tool_calls.entry(delta.index).or_default();
                            if let Some(id) = delta.id {
                                call.id.push_str(&id);
                            }
                            if let Some(function) = delta.function {
                                if let Some(name) = function.name {
                                    call.name.push_str(&name);
                                }
                                if let Some(arguments) = function.arguments {
                                    call.arguments.push_str(&arguments);
                                }
                            }
                        }
                        if let Some(reason) = choice.finish_reason {
                            finish_reason = decode_finish_reason(Some(reason.as_str()));
                        }
                    }
                }
            }

            if !buffer.iter().all(u8::is_ascii_whitespace) {
                return Err(AiError::Provider(
                    "provider stream ended with an incomplete event".to_string(),
                ))?;
            }
            let response = streamed_response(text, tool_calls, finish_reason, usage)?;
            yield AiEvent::ModelCompleted {
                run_id,
                step: 1,
                response,
            };
        })
    }
}

fn encode_request(model: &str, request: ModelRequest, stream: bool) -> Result<Value, AiError> {
    if request.messages.is_empty() {
        return Err(AiError::InvalidRequest(
            "model request requires at least one message".to_string(),
        ));
    }
    let messages = encode_messages(request.messages)?;
    let tools = request
        .tools
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.name,
                    "description": tool.description,
                    "parameters": tool.parameters,
                }
            })
        })
        .collect::<Vec<_>>();
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": stream,
    });
    let object = body
        .as_object_mut()
        .expect("chat completion request is an object");
    if !tools.is_empty() {
        object.insert("tools".to_string(), Value::Array(tools));
    }
    if let Some(temperature) = request.options.temperature {
        object.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(max_tokens) = request.options.max_tokens {
        object.insert("max_tokens".to_string(), json!(max_tokens));
    }
    if !request.options.stop.is_empty() {
        object.insert("stop".to_string(), json!(request.options.stop));
    }
    if let Some(response_format) = request.options.response_format {
        object.insert("response_format".to_string(), response_format);
    }
    if stream {
        object.insert("stream_options".to_string(), json!({"include_usage": true}));
    }
    Ok(body)
}

fn encode_messages(messages: Vec<Message>) -> Result<Vec<Value>, AiError> {
    let mut encoded = Vec::with_capacity(messages.len());
    for message in messages {
        match message.role {
            MessageRole::System | MessageRole::User => {
                encoded.push(json!({
                    "role": role_name(message.role),
                    "content": text_content(&message.content)?,
                }));
            }
            MessageRole::Assistant => {
                let content = text_content_optional(&message.content)?;
                let calls = message
                    .tool_calls()
                    .map(|call| {
                        json!({
                            "id": call.id,
                            "type": "function",
                            "function": {
                                "name": call.name,
                                "arguments": serde_json::to_string(&call.arguments)
                                    .unwrap_or_else(|_| "{}".to_string()),
                            }
                        })
                    })
                    .collect::<Vec<_>>();
                let mut value = json!({
                    "role": "assistant",
                    "content": content,
                });
                if !calls.is_empty() {
                    value
                        .as_object_mut()
                        .expect("assistant message is an object")
                        .insert("tool_calls".to_string(), Value::Array(calls));
                }
                encoded.push(value);
            }
            MessageRole::Tool => {
                for block in message.content {
                    let ContentBlock::ToolResult {
                        tool_call_id,
                        output,
                        ..
                    } = block
                    else {
                        return Err(AiError::InvalidRequest(
                            "tool messages may contain only tool results".to_string(),
                        ));
                    };
                    encoded.push(json!({
                        "role": "tool",
                        "tool_call_id": tool_call_id,
                        "content": value_as_text(output),
                    }));
                }
            }
        }
    }
    Ok(encoded)
}

fn text_content(content: &[ContentBlock]) -> Result<String, AiError> {
    text_content_optional(content)?.ok_or_else(|| {
        AiError::InvalidRequest("system and user messages require text content".to_string())
    })
}

fn text_content_optional(content: &[ContentBlock]) -> Result<Option<String>, AiError> {
    let mut text = String::new();
    for block in content {
        match block {
            ContentBlock::Text { text: value } => text.push_str(value),
            ContentBlock::ToolCall { .. } => {}
            ContentBlock::ToolResult { .. } => {
                return Err(AiError::InvalidRequest(
                    "tool results require a tool-role message".to_string(),
                ));
            }
        }
    }
    Ok((!text.is_empty()).then_some(text))
}

fn value_as_text(value: Value) -> String {
    match value {
        Value::String(value) => value,
        value => serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string()),
    }
}

fn role_name(role: MessageRole) -> &'static str {
    match role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    }
}

fn validate_status(response: &reqwest::Response) -> Result<(), AiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after_seconds = response
            .headers()
            .get(RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1)
            .max(1);
        return Err(AiError::RateLimited {
            retry_after_seconds,
        });
    }
    if status.is_server_error() || status == StatusCode::REQUEST_TIMEOUT {
        return Err(AiError::ProviderUnavailable(format!(
            "provider returned status {}",
            status.as_u16()
        )));
    }
    Err(AiError::Provider(format!(
        "provider rejected the request with status {}",
        status.as_u16()
    )))
}

fn map_reqwest_error(error: reqwest::Error) -> AiError {
    if error.is_timeout() {
        AiError::ProviderUnavailable("provider request timed out".to_string())
    } else if error.is_connect() {
        AiError::ProviderUnavailable("provider connection failed".to_string())
    } else {
        AiError::ProviderUnavailable("provider request failed".to_string())
    }
}

fn decode_response(payload: ChatCompletionResponse) -> Result<ModelResponse, AiError> {
    let choice = payload
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| AiError::Provider("provider returned no choices".to_string()))?;
    let message = decode_message(choice.message)?;
    Ok(ModelResponse {
        message,
        usage: payload.usage.map(Into::into).unwrap_or_default(),
        finish_reason: decode_finish_reason(choice.finish_reason.as_deref()),
    })
}

fn decode_message(message: ChatCompletionMessage) -> Result<Message, AiError> {
    let mut content = Vec::new();
    if let Some(text) = message.content.filter(|text| !text.is_empty()) {
        content.push(ContentBlock::Text { text });
    }
    for call in message.tool_calls {
        let arguments = serde_json::from_str(&call.function.arguments).map_err(|_| {
            AiError::Provider(format!(
                "provider returned invalid arguments for tool `{}`",
                call.function.name
            ))
        })?;
        content.push(ContentBlock::ToolCall {
            call: ToolCall::new(call.id, call.function.name, arguments),
        });
    }
    Ok(Message::new(MessageRole::Assistant, content))
}

fn decode_finish_reason(reason: Option<&str>) -> FinishReason {
    match reason {
        Some("tool_calls" | "function_call") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::ContentFilter,
        Some("stop") | None => FinishReason::Stop,
        Some(_) => FinishReason::Other,
    }
}

fn streamed_response(
    text: String,
    tool_calls: BTreeMap<usize, ToolCallAccumulator>,
    finish_reason: FinishReason,
    usage: ModelUsage,
) -> Result<ModelResponse, AiError> {
    let mut content = Vec::new();
    if !text.is_empty() {
        content.push(ContentBlock::Text { text });
    }
    for (_, call) in tool_calls {
        if call.id.is_empty() || call.name.is_empty() {
            return Err(AiError::Provider(
                "provider returned an incomplete streamed tool call".to_string(),
            ));
        }
        let arguments = serde_json::from_str(&call.arguments).map_err(|_| {
            AiError::Provider(format!(
                "provider returned invalid arguments for tool `{}`",
                call.name
            ))
        })?;
        content.push(ContentBlock::ToolCall {
            call: ToolCall::new(call.id, call.name, arguments),
        });
    }
    Ok(ModelResponse {
        message: Message::new(MessageRole::Assistant, content),
        usage,
        finish_reason,
    })
}

fn take_sse_event(buffer: &mut Vec<u8>) -> Option<Vec<u8>> {
    let lf = buffer.windows(2).position(|window| window == b"\n\n");
    let crlf = buffer.windows(4).position(|window| window == b"\r\n\r\n");
    let (index, delimiter_len) = match (lf, crlf) {
        (Some(left), Some(right)) if left <= right => (left, 2),
        (Some(_), Some(right)) => (right, 4),
        (Some(left), None) => (left, 2),
        (None, Some(right)) => (right, 4),
        (None, None) => return None,
    };
    let event = buffer[..index].to_vec();
    buffer.drain(..index + delimiter_len);
    Some(event)
}

fn sse_data(event: &[u8]) -> Result<String, AiError> {
    let event = std::str::from_utf8(event)
        .map_err(|_| AiError::Provider("provider stream was not UTF-8".to_string()))?;
    Ok(event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n"))
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<ChatCompletionChoice>,
    usage: Option<ChatUsage>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChoice {
    message: ChatCompletionMessage,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionMessage {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCall>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCall {
    id: String,
    function: ChatFunctionCall,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionCall {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct ChatUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
}

impl From<ChatUsage> for ModelUsage {
    fn from(value: ChatUsage) -> Self {
        Self {
            input_tokens: value.prompt_tokens,
            output_tokens: value.completion_tokens,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChatStreamChoice>,
    usage: Option<ChatUsage>,
    error: Option<ChatStreamError>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamChoice {
    #[serde(default)]
    delta: ChatStreamDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct ChatStreamDelta {
    content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ChatToolCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatToolCallDelta {
    index: usize,
    id: Option<String>,
    function: Option<ChatFunctionCallDelta>,
}

#[derive(Debug, Deserialize)]
struct ChatFunctionCallDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatStreamError {
    code: Option<String>,
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use super::*;
    use crate::{GenerationOptions, ToolDefinition};

    fn provider_config(base_url: String) -> AiProviderConfig {
        AiProviderConfig {
            kind: roze_config::AiProviderKind::OpenaiCompatible,
            base_url,
            api_key: Some("test-secret".to_string()),
            model: "test-model".to_string(),
            timeout_ms: 2_000,
        }
    }

    fn serve_once(body: String, content_type: &'static str) -> (String, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            let (header_end, content_length) = loop {
                let read = stream.read(&mut chunk).expect("read request");
                assert!(read > 0, "request ended before headers");
                request.extend_from_slice(&chunk[..read]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..index]).expect("headers");
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                        .unwrap_or(0);
                    break (index + 4, content_length);
                }
            };
            while request.len() < header_end + content_length {
                let read = stream.read(&mut chunk).expect("read body");
                assert!(read > 0, "request ended before body");
                request.extend_from_slice(&chunk[..read]);
            }
            let _ = sender.send(String::from_utf8(request).expect("request UTF-8"));
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
        });
        (format!("http://{address}/v1"), receiver)
    }

    #[tokio::test]
    async fn invokes_chat_completions_with_tools_and_context_headers() {
        let response = json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-1",
                        "type": "function",
                        "function": {
                            "name": "lookup",
                            "arguments": "{\"id\":7}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }],
            "usage": {"prompt_tokens": 8, "completion_tokens": 3}
        })
        .to_string();
        let (base_url, request) = serve_once(response, "application/json");
        let model = OpenAiCompatibleModel::from_config(&provider_config(base_url)).unwrap();
        let context = Context::background_with_request_id_and_trace_id("request-ai", "trace-ai");
        let result = model
            .invoke(
                &context,
                ModelRequest {
                    messages: vec![Message::user("find 7")],
                    tools: vec![ToolDefinition::new(
                        "lookup",
                        "Looks up one value",
                        json!({"type": "object"}),
                    )],
                    options: GenerationOptions::default(),
                },
            )
            .await
            .unwrap();

        assert_eq!(result.finish_reason, FinishReason::ToolCalls);
        assert_eq!(result.usage.input_tokens, 8);
        assert_eq!(
            result.message.tool_calls().next().unwrap().arguments,
            json!({"id": 7})
        );
        let request = request.recv().expect("request");
        assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-request-id: request-ai")));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: bearer test-secret")));
        let body = request.split("\r\n\r\n").nth(1).expect("request body");
        let body: Value = serde_json::from_str(body).expect("JSON body");
        assert_eq!(body["model"], "test-model");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup");
    }

    #[tokio::test]
    async fn streams_text_deltas_and_a_completed_response() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hel\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":1}}\n\n",
            "data: [DONE]\n\n"
        )
        .to_string();
        let (base_url, _) = serve_once(body, "text/event-stream");
        let model = OpenAiCompatibleModel::from_config(&provider_config(base_url)).unwrap();
        let context =
            Context::background_with_request_id_and_trace_id("request-stream", "trace-stream");
        let events = model
            .stream(
                &context,
                ModelRequest {
                    messages: vec![Message::user("hello")],
                    tools: Vec::new(),
                    options: GenerationOptions::default(),
                },
            )
            .collect::<Vec<_>>()
            .await;
        let events = events.into_iter().collect::<Result<Vec<_>, _>>().unwrap();

        assert!(matches!(
            events.as_slice(),
            [
                AiEvent::MessageDelta { delta: first, .. },
                AiEvent::MessageDelta { delta: second, .. },
                AiEvent::ModelCompleted { response, .. }
            ] if first == "hel"
                && second == "lo"
                && response.message == Message::assistant("hello")
                && response.usage.output_tokens == 1
        ));
    }

    #[test]
    fn debug_output_redacts_the_api_key() {
        let model = OpenAiCompatibleModel::from_config(&provider_config(
            "https://example.com/v1".to_string(),
        ))
        .unwrap();
        let debug = format!("{model:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("test-secret"));
    }
}
