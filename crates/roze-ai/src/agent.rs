use std::sync::Arc;

use roze_context::Context;
use uuid::Uuid;

use crate::{
    tool::check_context, AiError, AiEvent, ChatModel, GenerationOptions, Message, ModelRequest,
    ModelUsage, ToolRegistry,
};

/// Bounded options for one agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOptions {
    pub max_steps: usize,
    pub generation: GenerationOptions,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            max_steps: 8,
            generation: GenerationOptions::default(),
        }
    }
}

/// Complete output from one bounded agent run.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentOutput {
    pub run_id: String,
    pub message: Message,
    pub history: Vec<Message>,
    pub steps: usize,
    pub usage: ModelUsage,
    pub events: Vec<AiEvent>,
}

/// Minimal tool-calling agent over a model and a tool registry.
pub struct Agent {
    model_name: String,
    model: Arc<dyn ChatModel>,
    tools: ToolRegistry,
    options: AgentOptions,
}

impl Agent {
    pub fn new(
        model_name: impl Into<String>,
        model: Arc<dyn ChatModel>,
        tools: ToolRegistry,
        options: AgentOptions,
    ) -> Result<Self, AiError> {
        if options.max_steps == 0 {
            return Err(AiError::InvalidRequest(
                "AI agent max_steps must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            model_name: model_name.into(),
            model,
            tools,
            options,
        })
    }

    pub async fn invoke(
        &self,
        context: &Context,
        messages: impl IntoIterator<Item = Message>,
    ) -> Result<AgentOutput, AiError> {
        check_context(context)?;
        let run_id = Uuid::now_v7().to_string();
        let mut history = messages.into_iter().collect::<Vec<_>>();
        if history.is_empty() {
            return Err(AiError::InvalidRequest(
                "AI agent requires at least one message".to_string(),
            ));
        }

        let mut events = vec![AiEvent::RunStarted {
            run_id: run_id.clone(),
            model: self.model_name.clone(),
        }];
        let mut usage = ModelUsage::default();

        for step in 1..=self.options.max_steps {
            check_context(context)?;
            let response = self
                .model
                .invoke(
                    context,
                    ModelRequest {
                        messages: history.clone(),
                        tools: self.tools.definitions(),
                        options: self.options.generation.clone(),
                    },
                )
                .await?;
            check_context(context)?;
            usage.add_assign(response.usage);
            events.push(AiEvent::ModelCompleted {
                run_id: run_id.clone(),
                step,
                response: response.clone(),
            });

            let tool_calls = response.message.tool_calls().cloned().collect::<Vec<_>>();
            let final_message = response.message.clone();
            history.push(response.message);

            if tool_calls.is_empty() {
                events.push(AiEvent::RunCompleted {
                    run_id: run_id.clone(),
                    steps: step,
                    usage,
                });
                return Ok(AgentOutput {
                    run_id,
                    message: final_message,
                    history,
                    steps: step,
                    usage,
                    events,
                });
            }

            for call in tool_calls {
                events.push(AiEvent::ToolStarted {
                    run_id: run_id.clone(),
                    step,
                    call: call.clone(),
                });
                let output = self.tools.invoke(context, &call).await?;
                events.push(AiEvent::ToolCompleted {
                    run_id: run_id.clone(),
                    step,
                    call: call.clone(),
                    output: output.clone(),
                });
                history.push(Message::tool_result(&call, output));
            }
        }

        Err(AiError::MaxStepsExceeded {
            max_steps: self.options.max_steps,
        })
    }
}
