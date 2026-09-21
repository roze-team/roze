use std::{collections::BTreeMap, fmt, time::Duration};

use reqwest::header::AUTHORIZATION;
use roze_config::AiProviderConfig;
use roze_context::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    openai_compatible::{map_reqwest_error, validate_status},
    tool::check_context,
    AiError, ModelUsage,
};

/// A typed question accepted by TypeSafe System One models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SystemOneQuestion {
    Noul {
        instructions: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        criteria: Option<BTreeMap<String, Value>>,
    },
    Choice {
        instructions: Value,
        criteria: BTreeMap<String, Value>,
    },
    Score {
        instructions: Value,
        criteria: Vec<Value>,
    },
}

impl SystemOneQuestion {
    pub fn noul(instructions: impl Into<Value>) -> Self {
        Self::Noul {
            instructions: instructions.into(),
            criteria: None,
        }
    }

    pub fn choice(
        instructions: impl Into<Value>,
        criteria: impl IntoIterator<Item = (impl Into<String>, impl Into<Value>)>,
    ) -> Self {
        Self::Choice {
            instructions: instructions.into(),
            criteria: criteria
                .into_iter()
                .map(|(name, description)| (name.into(), description.into()))
                .collect(),
        }
    }

    pub fn score(
        instructions: impl Into<Value>,
        criteria: impl IntoIterator<Item = impl Into<Value>>,
    ) -> Self {
        Self::Score {
            instructions: instructions.into(),
            criteria: criteria.into_iter().map(Into::into).collect(),
        }
    }

    fn validate(&self) -> Result<(), AiError> {
        let (instructions, criteria_len, range) = match self {
            Self::Noul {
                instructions,
                criteria,
            } => {
                if let Some(criteria) = criteria {
                    for description in criteria.values() {
                        validate_structured_value("Noul criterion", description)?;
                    }
                }
                (instructions, None, None)
            }
            Self::Choice {
                instructions,
                criteria,
            } => {
                for (name, description) in criteria {
                    if name.trim().is_empty() {
                        return Err(AiError::InvalidRequest(
                            "System One Choice option cannot be empty".to_string(),
                        ));
                    }
                    if !description.is_null() {
                        validate_structured_value("Choice criterion", description)?;
                    }
                }
                (instructions, Some(criteria.len()), Some(2..=255))
            }
            Self::Score {
                instructions,
                criteria,
            } => {
                for description in criteria {
                    validate_structured_value("Score criterion", description)?;
                }
                (instructions, Some(criteria.len()), Some(2..=10))
            }
        };
        validate_structured_value("question instructions", instructions)?;
        if let (Some(len), Some(range)) = (criteria_len, range) {
            if !range.contains(&len) {
                return Err(AiError::InvalidRequest(format!(
                    "System One question criteria count must be between {} and {}",
                    range.start(),
                    range.end()
                )));
            }
        }
        Ok(())
    }
}

/// One TypeSafe System One evaluation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemOneRequest {
    pub state: Value,
    pub questions: BTreeMap<String, SystemOneQuestion>,
}

impl SystemOneRequest {
    pub fn new(
        state: impl Into<Value>,
        questions: impl IntoIterator<Item = (impl Into<String>, SystemOneQuestion)>,
    ) -> Self {
        Self {
            state: state.into(),
            questions: questions
                .into_iter()
                .map(|(name, question)| (name.into(), question))
                .collect(),
        }
    }

    fn validate(&self) -> Result<(), AiError> {
        validate_structured_value("state", &self.state)?;
        if self.questions.is_empty() {
            return Err(AiError::InvalidRequest(
                "System One request requires at least one question".to_string(),
            ));
        }
        for (name, question) in &self.questions {
            if name.trim().is_empty() {
                return Err(AiError::InvalidRequest(
                    "System One question name cannot be empty".to_string(),
                ));
            }
            question.validate()?;
        }
        Ok(())
    }
}

/// A typed answer returned by TypeSafe System One.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SystemOneAnswer {
    Noul {
        noul: f64,
    },
    Choice {
        choice: String,
        confidence: f64,
        probabilities: BTreeMap<String, f64>,
    },
    Score {
        score: f64,
        confidence: f64,
        legend: BTreeMap<String, String>,
        probabilities: BTreeMap<String, f64>,
    },
}

/// The model output and token usage returned by TypeSafe System One.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemOneResponse {
    pub model: String,
    pub answers: BTreeMap<String, SystemOneAnswer>,
    pub usage: ModelUsage,
}

#[derive(Clone)]
pub struct TypeSafeSystemOneModel {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    model: String,
}

impl TypeSafeSystemOneModel {
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

    pub async fn evaluate(
        &self,
        context: &Context,
        request: SystemOneRequest,
    ) -> Result<SystemOneResponse, AiError> {
        check_context(context)?;
        request.validate()?;
        let mut body = serde_json::to_value(request)
            .map_err(|_| AiError::Internal("failed to encode System One request".to_string()))?;
        body.as_object_mut()
            .expect("System One request is an object")
            .insert("model".to_string(), Value::String(self.model.clone()));
        let mut builder = self
            .client
            .post(format!("{}/systemone", self.base_url))
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
        response
            .json::<SystemOneResponse>()
            .await
            .map_err(|_| AiError::Provider("provider returned invalid JSON".to_string()))
    }
}

impl fmt::Debug for TypeSafeSystemOneModel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypeSafeSystemOneModel")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "[REDACTED]"))
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

fn validate_structured_value(name: &str, value: &Value) -> Result<(), AiError> {
    if matches!(value, Value::String(_) | Value::Array(_) | Value::Object(_)) {
        return Ok(());
    }
    Err(AiError::InvalidRequest(format!(
        "System One {name} must be a string, object, or array"
    )))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::mpsc,
        thread,
    };

    use serde_json::json;

    use super::*;

    fn serve_once(body: String) -> (String, mpsc::Receiver<String>) {
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
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().expect("content length"))
                        })
                        .unwrap_or(0);
                    break (index + 4, length);
                }
            };
            while request.len() < header_end + content_length {
                let read = stream.read(&mut chunk).expect("read body");
                assert!(read > 0, "request ended before body");
                request.extend_from_slice(&chunk[..read]);
            }
            sender
                .send(String::from_utf8(request).expect("request UTF-8"))
                .expect("send request");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .expect("write response");
        });
        (format!("http://{address}/v1"), receiver)
    }

    #[tokio::test]
    async fn evaluates_typed_questions_with_context_headers() {
        let response = json!({
            "model": "jev-1.13.0",
            "answers": {
                "urgent": {"type": "noul", "noul": 0.95}
            },
            "usage": {"input_tokens": 12, "output_tokens": 3}
        })
        .to_string();
        let (base_url, received) = serve_once(response);
        let model = TypeSafeSystemOneModel::from_config(&AiProviderConfig {
            kind: roze_config::AiProviderKind::TypesafeSystemOne,
            base_url,
            api_key: Some("test-secret".to_string()),
            model: "jev-latest".to_string(),
            timeout_ms: 2_000,
        })
        .expect("model");
        let context = Context::background_with_request_id_and_trace_id("request-ai", "trace-ai");
        let result = model
            .evaluate(
                &context,
                SystemOneRequest::new(
                    "Help! This is urgent.",
                    [(
                        "urgent",
                        SystemOneQuestion::noul("Does this convey urgency?"),
                    )],
                ),
            )
            .await
            .expect("evaluate");

        assert_eq!(result.model, "jev-1.13.0");
        assert_eq!(result.usage.input_tokens, 12);
        assert_eq!(
            result.answers["urgent"],
            SystemOneAnswer::Noul { noul: 0.95 }
        );
        let request = received.recv().expect("request");
        assert!(request.starts_with("POST /v1/systemone HTTP/1.1"));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-request-id: request-ai")));
        assert!(request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: bearer test-secret")));
        let body: Value =
            serde_json::from_str(request.split("\r\n\r\n").nth(1).expect("request body"))
                .expect("JSON body");
        assert_eq!(body["model"], "jev-latest");
        assert_eq!(body["questions"]["urgent"]["type"], "noul");
    }

    #[test]
    fn rejects_invalid_score_shape_and_redacts_debug() {
        let request = SystemOneRequest::new(
            "state",
            [("score", SystemOneQuestion::score("Rate it", ["only one"]))],
        );
        assert!(request.validate().is_err());

        let model = TypeSafeSystemOneModel::from_config(&AiProviderConfig {
            kind: roze_config::AiProviderKind::TypesafeSystemOne,
            base_url: "https://api.typesafe.ai/v1".to_string(),
            api_key: Some("must-not-leak".to_string()),
            model: "jev-latest".to_string(),
            timeout_ms: 30_000,
        })
        .expect("model");
        let debug = format!("{model:?}");
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("must-not-leak"));
    }
}
