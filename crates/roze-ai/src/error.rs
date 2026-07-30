use roze_error::RozeError;
use thiserror::Error;

/// Stable error categories exposed by the provider-neutral AI runtime.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AiError {
    #[error("invalid AI request: {0}")]
    InvalidRequest(String),
    #[error("AI model not found: {0}")]
    ModelNotFound(String),
    #[error("AI tool not found: {0}")]
    ToolNotFound(String),
    #[error("AI tool permission denied: {0}")]
    PermissionDenied(String),
    #[error("AI request was cancelled")]
    Cancelled,
    #[error("AI request deadline exceeded")]
    DeadlineExceeded,
    #[error("AI provider unavailable: {0}")]
    ProviderUnavailable(String),
    #[error("AI provider rate limited; retry after {retry_after_seconds}s")]
    RateLimited { retry_after_seconds: u64 },
    #[error("AI provider failed: {0}")]
    Provider(String),
    #[error("AI tool failed: {0}")]
    Tool(String),
    #[error("AI agent exceeded its maximum of {max_steps} steps")]
    MaxStepsExceeded { max_steps: usize },
    #[error("invalid AI workflow: {0}")]
    InvalidWorkflow(String),
    #[error("AI workflow node `{node}` failed: {message}")]
    WorkflowNode { node: String, message: String },
    #[error("AI workflow stream exceeded its maximum of {max_chunks} chunks")]
    WorkflowStreamLimit { max_chunks: usize },
    #[error("AI workflow checkpoint failed: {0}")]
    Checkpoint(String),
    #[error("AI workflow checkpoint not found: {0}")]
    CheckpointNotFound(String),
    #[error("AI workflow checkpoint scope does not match the current request")]
    CheckpointScopeMismatch,
    #[error("AI workflow checkpoint revision `{actual}` does not match `{expected}`")]
    CheckpointRevisionMismatch { expected: String, actual: String },
    #[error("invalid AI agent team: {0}")]
    InvalidTeam(String),
    #[error("AI agent not found: {0}")]
    AgentNotFound(String),
    #[error("AI retrieval failed: {0}")]
    Retrieval(String),
    #[error("AI indexing failed: {0}")]
    Indexing(String),
    #[error("AI prompt rendering failed: {0}")]
    Prompt(String),
    #[error("AI runtime error: {0}")]
    Internal(String),
}

impl From<AiError> for RozeError {
    fn from(error: AiError) -> Self {
        match error {
            AiError::InvalidRequest(message) => Self::BadRequest(message),
            AiError::ModelNotFound(name) => Self::NotFound(format!("AI model `{name}`")),
            AiError::ToolNotFound(name) => Self::NotFound(format!("AI tool `{name}`")),
            AiError::PermissionDenied(_) => Self::Forbidden,
            AiError::Cancelled => Self::Unavailable("AI request was cancelled".to_string()),
            AiError::DeadlineExceeded => {
                Self::Unavailable("AI request deadline exceeded".to_string())
            }
            AiError::ProviderUnavailable(message) => Self::Unavailable(message),
            AiError::RateLimited {
                retry_after_seconds,
            } => Self::RateLimited {
                retry_after_seconds,
            },
            AiError::Provider(message) | AiError::Tool(message) | AiError::Internal(message) => {
                Self::Internal(message)
            }
            AiError::MaxStepsExceeded { max_steps } => Self::Internal(format!(
                "AI agent exceeded its maximum of {max_steps} steps"
            )),
            AiError::InvalidWorkflow(message) | AiError::Prompt(message) => {
                Self::BadRequest(message)
            }
            AiError::WorkflowNode { node, message } => {
                Self::Internal(format!("AI workflow node `{node}` failed: {message}"))
            }
            AiError::WorkflowStreamLimit { max_chunks } => Self::Internal(format!(
                "AI workflow stream exceeded its maximum of {max_chunks} chunks"
            )),
            AiError::Checkpoint(message) => Self::Unavailable(message),
            AiError::CheckpointNotFound(run_id) => {
                Self::NotFound(format!("AI workflow checkpoint `{run_id}`"))
            }
            AiError::CheckpointScopeMismatch => Self::Forbidden,
            AiError::CheckpointRevisionMismatch { expected, actual } => Self::BadRequest(format!(
                "AI workflow checkpoint revision `{actual}` does not match `{expected}`"
            )),
            AiError::InvalidTeam(message) => Self::BadRequest(message),
            AiError::AgentNotFound(name) => Self::NotFound(format!("AI agent `{name}`")),
            AiError::Retrieval(message) | AiError::Indexing(message) => Self::Unavailable(message),
        }
    }
}
