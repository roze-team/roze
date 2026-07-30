use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use futures_util::future::join_all;
use roze_context::Context;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    tool::check_context, Agent, AgentOutput, AiError, Message, ModelUsage, Tool, ToolDefinition,
};

#[async_trait]
pub trait AgentExecutor: Send + Sync {
    async fn invoke(
        &self,
        context: &Context,
        messages: Vec<Message>,
    ) -> Result<AgentOutput, AiError>;
}

#[async_trait]
impl AgentExecutor for Agent {
    async fn invoke(
        &self,
        context: &Context,
        messages: Vec<Message>,
    ) -> Result<AgentOutput, AiError> {
        Agent::invoke(self, context, messages).await
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentTask {
    pub id: String,
    pub agent: String,
    pub messages: Vec<Message>,
}

impl AgentTask {
    pub fn new(
        id: impl Into<String>,
        agent: impl Into<String>,
        messages: impl IntoIterator<Item = Message>,
    ) -> Self {
        Self {
            id: id.into(),
            agent: agent.into(),
            messages: messages.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TeamExecutionMode {
    #[default]
    Sequential,
    Parallel,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentTaskOutput {
    pub task_id: String,
    pub agent: String,
    pub output: AgentOutput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TeamOutput {
    pub tasks: Vec<AgentTaskOutput>,
    pub usage: ModelUsage,
}

#[derive(Clone)]
pub struct AgentTeam {
    agents: BTreeMap<String, Arc<dyn AgentExecutor>>,
    max_tasks: usize,
}

impl AgentTeam {
    pub fn new(max_tasks: usize) -> Result<Self, AiError> {
        if max_tasks == 0 {
            return Err(AiError::InvalidTeam(
                "max_tasks must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            agents: BTreeMap::new(),
            max_tasks,
        })
    }

    pub fn register<A>(&mut self, name: impl Into<String>, agent: A) -> Result<(), AiError>
    where
        A: AgentExecutor + 'static,
    {
        self.register_arc(name, Arc::new(agent))
    }

    pub fn register_arc(
        &mut self,
        name: impl Into<String>,
        agent: Arc<dyn AgentExecutor>,
    ) -> Result<(), AiError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(AiError::InvalidTeam(
                "agent name cannot be empty".to_string(),
            ));
        }
        if self.agents.contains_key(&name) {
            return Err(AiError::InvalidTeam(format!(
                "agent `{name}` is already registered"
            )));
        }
        self.agents.insert(name, agent);
        Ok(())
    }

    pub async fn run(
        &self,
        context: &Context,
        tasks: Vec<AgentTask>,
        mode: TeamExecutionMode,
    ) -> Result<TeamOutput, AiError> {
        check_context(context)?;
        self.validate_tasks(&tasks)?;
        let outputs = match mode {
            TeamExecutionMode::Sequential => {
                let mut outputs = Vec::with_capacity(tasks.len());
                for task in tasks {
                    outputs.push(self.run_task(context, task).await?);
                }
                outputs
            }
            TeamExecutionMode::Parallel => {
                let futures = tasks.into_iter().map(|task| self.run_task(context, task));
                join_all(futures)
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        check_context(context)?;
        let mut usage = ModelUsage::default();
        for output in &outputs {
            usage.add_assign(output.output.usage);
        }
        Ok(TeamOutput {
            tasks: outputs,
            usage,
        })
    }

    /// Delegates one bounded task to a registered agent.
    pub async fn delegate(
        &self,
        context: &Context,
        agent: impl Into<String>,
        messages: impl IntoIterator<Item = Message>,
    ) -> Result<AgentTaskOutput, AiError> {
        let agent = agent.into();
        self.run_task(
            context,
            AgentTask::new(format!("delegate-{agent}"), agent, messages),
        )
        .await
    }

    /// Exposes registered agents as one permission-aware model tool.
    pub fn delegation_tool(
        &self,
        name: impl Into<String>,
        description: impl Into<String>,
        required_permissions: impl IntoIterator<Item = impl Into<String>>,
    ) -> DelegationTool {
        let agents = self.agents.keys().cloned().collect::<Vec<_>>();
        let definition = ToolDefinition::new(
            name,
            description,
            json!({
                "type": "object",
                "properties": {
                    "agent": {
                        "type": "string",
                        "enum": agents,
                    },
                    "prompt": {
                        "type": "string",
                    },
                },
                "required": ["agent", "prompt"],
                "additionalProperties": false,
            }),
        )
        .with_permissions(required_permissions);
        DelegationTool {
            team: self.clone(),
            definition,
        }
    }

    pub fn len(&self) -> usize {
        self.agents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }

    fn validate_tasks(&self, tasks: &[AgentTask]) -> Result<(), AiError> {
        if tasks.is_empty() {
            return Err(AiError::InvalidTeam(
                "agent team requires at least one task".to_string(),
            ));
        }
        if tasks.len() > self.max_tasks {
            return Err(AiError::InvalidTeam(format!(
                "agent team task count {} exceeds maximum {}",
                tasks.len(),
                self.max_tasks
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        for task in tasks {
            if task.id.trim().is_empty() {
                return Err(AiError::InvalidTeam(
                    "agent task id cannot be empty".to_string(),
                ));
            }
            if !ids.insert(task.id.as_str()) {
                return Err(AiError::InvalidTeam(format!(
                    "duplicate agent task id `{}`",
                    task.id
                )));
            }
            if task.messages.is_empty() {
                return Err(AiError::InvalidTeam(format!(
                    "agent task `{}` requires at least one message",
                    task.id
                )));
            }
            if !self.agents.contains_key(&task.agent) {
                return Err(AiError::AgentNotFound(task.agent.clone()));
            }
        }
        Ok(())
    }

    async fn run_task(
        &self,
        context: &Context,
        task: AgentTask,
    ) -> Result<AgentTaskOutput, AiError> {
        check_context(context)?;
        let agent = self
            .agents
            .get(&task.agent)
            .ok_or_else(|| AiError::AgentNotFound(task.agent.clone()))?;
        let output = agent.invoke(context, task.messages).await?;
        check_context(context)?;
        Ok(AgentTaskOutput {
            task_id: task.id,
            agent: task.agent,
            output,
        })
    }
}

/// A model-callable gateway to a bounded set of registered agents.
pub struct DelegationTool {
    team: AgentTeam,
    definition: ToolDefinition,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelegationArguments {
    agent: String,
    prompt: String,
}

#[async_trait]
impl Tool for DelegationTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn invoke(&self, context: &Context, arguments: Value) -> Result<Value, AiError> {
        let arguments =
            serde_json::from_value::<DelegationArguments>(arguments).map_err(|error| {
                AiError::InvalidTeam(format!("invalid delegation arguments: {error}"))
            })?;
        if arguments.prompt.trim().is_empty() {
            return Err(AiError::InvalidTeam(
                "delegation prompt cannot be empty".to_string(),
            ));
        }
        let result = self
            .team
            .delegate(context, arguments.agent, [Message::user(arguments.prompt)])
            .await?;
        Ok(json!({
            "task_id": result.task_id,
            "agent": result.agent,
            "message": result.output.message,
            "steps": result.output.steps,
            "usage": result.output.usage,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentOptions, MockChatModel, ModelResponse, ToolCall, ToolRegistry};

    fn test_agent(answer: &str) -> Agent {
        Agent::new(
            "mock",
            Arc::new(MockChatModel::new([ModelResponse::text(answer)])),
            ToolRegistry::new(),
            AgentOptions::default(),
        )
        .expect("agent")
    }

    #[tokio::test]
    async fn team_runs_parallel_tasks_in_declaration_order() {
        let mut team = AgentTeam::new(4).expect("team");
        team.register("research", test_agent("research"))
            .expect("agent");
        team.register("review", test_agent("review"))
            .expect("agent");
        let tasks = vec![
            AgentTask::new("one", "research", [Message::user("research")]),
            AgentTask::new("two", "review", [Message::user("review")]),
        ];

        let output = team
            .run(&Context::background(), tasks, TeamExecutionMode::Parallel)
            .await
            .expect("run");
        assert_eq!(output.tasks[0].task_id, "one");
        assert_eq!(output.tasks[1].task_id, "two");
    }

    #[tokio::test]
    async fn team_enforces_task_bounds_and_registered_agents() {
        let team = AgentTeam::new(1).expect("team");
        let missing = team
            .run(
                &Context::background(),
                vec![AgentTask::new("one", "missing", [Message::user("work")])],
                TeamExecutionMode::Sequential,
            )
            .await;
        assert!(matches!(missing, Err(AiError::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn delegation_tool_routes_to_a_registered_agent() {
        let mut team = AgentTeam::new(2).expect("team");
        team.register("research", test_agent("delegated"))
            .expect("agent");
        let tool = team.delegation_tool("delegate", "Delegate work", ["ai.delegate"]);

        let output = tool
            .invoke(
                &Context::background(),
                json!({"agent": "research", "prompt": "investigate"}),
            )
            .await
            .expect("delegate");
        assert_eq!(output["agent"], "research");
        assert_eq!(output["message"]["content"][0]["text"], "delegated");
    }

    #[tokio::test]
    async fn delegation_tool_rejects_unknown_agents() {
        let team = AgentTeam::new(1).expect("team");
        let tool = team.delegation_tool("delegate", "Delegate work", Vec::<String>::new());
        let result = tool
            .invoke(
                &Context::background(),
                json!({"agent": "missing", "prompt": "investigate"}),
            )
            .await;
        assert!(matches!(result, Err(AiError::AgentNotFound(_))));
    }

    #[tokio::test]
    async fn delegation_tool_uses_standard_tool_permissions() {
        let mut team = AgentTeam::new(1).expect("team");
        team.register("research", test_agent("delegated"))
            .expect("agent");
        let mut tools = ToolRegistry::new();
        tools
            .register(team.delegation_tool("delegate", "Delegate work", ["ai.delegate"]))
            .expect("tool");
        let call = ToolCall::new(
            "call-1",
            "delegate",
            json!({"agent": "research", "prompt": "investigate"}),
        );

        let denied = tools.invoke(&Context::background(), &call).await;
        assert_eq!(
            denied,
            Err(AiError::PermissionDenied("delegate".to_string()))
        );
        tools
            .invoke(
                &Context::background().with_permissions(["ai.delegate"]),
                &call,
            )
            .await
            .expect("permitted");
    }
}
