use std::{collections::BTreeMap, sync::Arc};

use roze_config::{AiConfig, AiProviderKind};
use roze_context::Context;

use crate::{
    Agent, AgentOptions, AgentOutput, AiError, ChatModel, Message, OpenAiCompatibleModel, Tool,
    ToolRegistry, TypeSafeSystemOneModel,
};

/// Cloneable AI runtime intended for Roze `ApplicationExtensions`.
#[derive(Clone)]
pub struct AiRuntime {
    default_model: String,
    models: BTreeMap<String, Arc<dyn ChatModel>>,
    system_one_models: BTreeMap<String, Arc<TypeSafeSystemOneModel>>,
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
        let mut runtime = Self::new(&config.default_provider, chat_model_from_config(default)?)?;
        runtime.default_agent_options.max_steps = config.max_steps;
        for (name, provider) in &config.providers {
            if name == &config.default_provider {
                continue;
            }
            match provider.kind {
                AiProviderKind::OpenaiCompatible => {
                    runtime.register_model(name, chat_model_from_config(provider)?)?;
                }
                AiProviderKind::TypesafeSystemOne => {
                    runtime.register_system_one_model(
                        name,
                        Arc::new(TypeSafeSystemOneModel::from_config(provider)?),
                    )?;
                }
            }
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
            system_one_models: BTreeMap::new(),
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
        if self.models.contains_key(&name) || self.system_one_models.contains_key(&name) {
            return Err(AiError::InvalidRequest(format!(
                "AI model `{name}` is already registered"
            )));
        }
        self.models.insert(name, model);
        Ok(())
    }

    pub fn register_system_one_model(
        &mut self,
        name: impl Into<String>,
        model: Arc<TypeSafeSystemOneModel>,
    ) -> Result<(), AiError> {
        let name = name.into();
        validate_model_name(&name)?;
        if self.models.contains_key(&name) || self.system_one_models.contains_key(&name) {
            return Err(AiError::InvalidRequest(format!(
                "AI model `{name}` is already registered"
            )));
        }
        self.system_one_models.insert(name, model);
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

    pub fn system_one_model(&self, name: &str) -> Option<Arc<TypeSafeSystemOneModel>> {
        self.system_one_models.get(name).cloned()
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

    pub fn system_one_model_count(&self) -> usize {
        self.system_one_models.len()
    }

    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    pub fn default_agent_options(&self) -> &AgentOptions {
        &self.default_agent_options
    }
}

fn chat_model_from_config(
    config: &roze_config::AiProviderConfig,
) -> Result<Arc<dyn ChatModel>, AiError> {
    match config.kind {
        AiProviderKind::OpenaiCompatible => {
            Ok(Arc::new(OpenAiCompatibleModel::from_config(config)?))
        }
        AiProviderKind::TypesafeSystemOne => Err(AiError::InvalidRequest(
            "ai.default_provider must reference a chat model; TypeSafe System One is a decision model"
                .to_string(),
        )),
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

#[cfg(test)]
mod tests {
    use super::*;
    use roze_config::{AiProviderConfig, AiProviderKind};

    #[test]
    fn builds_chat_and_system_one_models_from_config() {
        let config = AiConfig {
            default_provider: "chat".to_string(),
            max_steps: 8,
            providers: BTreeMap::from([
                (
                    "chat".to_string(),
                    AiProviderConfig {
                        kind: AiProviderKind::OpenaiCompatible,
                        base_url: "https://api.openai.com/v1".to_string(),
                        api_key: None,
                        model: "gpt-5".to_string(),
                        timeout_ms: 30_000,
                    },
                ),
                (
                    "decisions".to_string(),
                    AiProviderConfig {
                        kind: AiProviderKind::TypesafeSystemOne,
                        base_url: "https://api.typesafe.ai/v1".to_string(),
                        api_key: None,
                        model: "jev-latest".to_string(),
                        timeout_ms: 30_000,
                    },
                ),
            ]),
        };

        let runtime = AiRuntime::from_config(&config).expect("runtime");
        assert_eq!(runtime.model_count(), 1);
        assert_eq!(runtime.system_one_model_count(), 1);
        assert!(runtime.system_one_model("decisions").is_some());
    }
}
