use std::{collections::BTreeMap, sync::Arc};

use roze_config::{AiConfig, AiProviderKind};
use roze_context::Context;

use crate::{
    Agent, AgentOptions, AgentOutput, AiError, ChatModel, Message, OpenAiCompatibleModel, Tool,
    ToolRegistry,
};

/// Cloneable AI runtime intended for Roze `ApplicationExtensions`.
#[derive(Clone)]
pub struct AiRuntime {
    default_model: String,
    models: BTreeMap<String, Arc<dyn ChatModel>>,
    tools: ToolRegistry,
    default_agent_options: AgentOptions,
}

impl AiRuntime {
    pub fn from_config(config: &AiConfig) -> Result<Self, AiError> {
        config
            .validate()
            .map_err(|error| AiError::InvalidRequest(error.to_string()))?;
        let default = config.default_provider_config().ok_or_else(|| {
            AiError::InvalidRequest(format!(
                "AI default provider `{}` is not configured",
                config.default_provider
            ))
        })?;
        let mut runtime = Self::new(&config.default_provider, model_from_config(default)?)?;
        runtime.default_agent_options.max_steps = config.max_steps;
        for (name, provider) in &config.providers {
            if name == &config.default_provider {
                continue;
            }
            runtime.register_model(name, model_from_config(provider)?)?;
        }
        Ok(runtime)
    }

    pub fn new(
        default_model: impl Into<String>,
        model: Arc<dyn ChatModel>,
    ) -> Result<Self, AiError> {
        let default_model = default_model.into();
        validate_model_name(&default_model)?;
        Ok(Self {
            default_model: default_model.clone(),
            models: BTreeMap::from([(default_model, model)]),
            tools: ToolRegistry::new(),
            default_agent_options: AgentOptions::default(),
        })
    }

    pub fn register_model(
        &mut self,
        name: impl Into<String>,
        model: Arc<dyn ChatModel>,
    ) -> Result<(), AiError> {
        let name = name.into();
        validate_model_name(&name)?;
        if self.models.contains_key(&name) {
            return Err(AiError::InvalidRequest(format!(
                "AI model `{name}` is already registered"
            )));
        }
        self.models.insert(name, model);
        Ok(())
    }

    pub fn register_tool<T>(&mut self, tool: T) -> Result<(), AiError>
    where
        T: Tool + 'static,
    {
        self.tools.register(tool)
    }

    pub fn register_tool_arc(&mut self, tool: Arc<dyn Tool>) -> Result<(), AiError> {
        self.tools.register_arc(tool)
    }

    pub fn model(&self, name: &str) -> Option<Arc<dyn ChatModel>> {
        self.models.get(name).cloned()
    }

    pub fn agent(&self, model_name: Option<&str>, options: AgentOptions) -> Result<Agent, AiError> {
        let model_name = model_name.unwrap_or(&self.default_model);
        let model = self
            .model(model_name)
            .ok_or_else(|| AiError::ModelNotFound(model_name.to_string()))?;
        Agent::new(model_name, model, self.tools.clone(), options)
    }

    pub async fn invoke(
        &self,
        context: &Context,
        messages: impl IntoIterator<Item = Message>,
    ) -> Result<AgentOutput, AiError> {
        self.agent(None, self.default_agent_options.clone())?
            .invoke(context, messages)
            .await
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    pub fn model_count(&self) -> usize {
        self.models.len()
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn default_agent_options(&self) -> &AgentOptions {
        &self.default_agent_options
    }
}

fn model_from_config(
    config: &roze_config::AiProviderConfig,
) -> Result<Arc<dyn ChatModel>, AiError> {
    match config.kind {
        AiProviderKind::OpenaiCompatible => {
            Ok(Arc::new(OpenAiCompatibleModel::from_config(config)?))
        }
    }
}

fn validate_model_name(name: &str) -> Result<(), AiError> {
    if name.trim().is_empty() {
        return Err(AiError::InvalidRequest(
            "AI model name cannot be empty".to_string(),
        ));
    }
    Ok(())
}
