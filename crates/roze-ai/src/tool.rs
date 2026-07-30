use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use roze_context::Context;
use serde_json::Value;

use crate::{AiError, ToolCall, ToolDefinition};

/// Application-owned tool implementation.
#[async_trait]
pub trait Tool: Send + Sync {
    fn definition(&self) -> ToolDefinition;

    async fn invoke(&self, context: &Context, arguments: Value) -> Result<Value, AiError>;
}

/// Immutable-at-run-time registry of application tools.
#[derive(Clone, Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<T>(&mut self, tool: T) -> Result<(), AiError>
    where
        T: Tool + 'static,
    {
        self.register_arc(Arc::new(tool))
    }

    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) -> Result<(), AiError> {
        let definition = tool.definition();
        validate_tool_definition(&definition)?;
        if self.tools.contains_key(&definition.name) {
            return Err(AiError::InvalidRequest(format!(
                "AI tool `{}` is already registered",
                definition.name
            )));
        }
        self.tools.insert(definition.name, tool);
        Ok(())
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools.values().map(|tool| tool.definition()).collect()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub async fn invoke(&self, context: &Context, call: &ToolCall) -> Result<Value, AiError> {
        check_context(context)?;
        let tool = self
            .get(&call.name)
            .ok_or_else(|| AiError::ToolNotFound(call.name.clone()))?;
        let definition = tool.definition();
        if !context.has_permissions(&definition.required_permissions) {
            return Err(AiError::PermissionDenied(call.name.clone()));
        }
        let output = tool.invoke(context, call.arguments.clone()).await?;
        check_context(context)?;
        Ok(output)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

fn validate_tool_definition(definition: &ToolDefinition) -> Result<(), AiError> {
    if definition.name.trim().is_empty() {
        return Err(AiError::InvalidRequest(
            "AI tool name cannot be empty".to_string(),
        ));
    }
    if !definition.parameters.is_object() {
        return Err(AiError::InvalidRequest(format!(
            "AI tool `{}` parameters must be a JSON object schema",
            definition.name
        )));
    }
    Ok(())
}

pub(crate) fn check_context(context: &Context) -> Result<(), AiError> {
    if context.cancelled() {
        return Err(AiError::Cancelled);
    }
    if context.is_expired() {
        return Err(AiError::DeadlineExceeded);
    }
    Ok(())
}
