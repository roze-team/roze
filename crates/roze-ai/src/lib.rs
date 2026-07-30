//! Provider-neutral AI application primitives for Roze services.
//!
//! This crate owns AI-specific semantics only. Configuration, service
//! lifecycle, permissions, storage, cache, messaging, governance, and
//! observability remain owned by their existing Roze modules.

mod agent;
mod compose;
mod error;
mod model;
mod openai_compatible;
mod prompt;
mod rag;
mod runtime;
mod schema;
mod storage_checkpoint;
mod team;
mod tool;

pub use agent::{Agent, AgentOptions, AgentOutput};
pub use compose::{
    CheckpointStore, CompiledGraph, FnNode, FnStreamNode, GraphBuilder, GraphTool,
    MemoryCheckpointStore, NodeFuture, NodeStream, PassthroughNode, WorkflowCheckpoint,
    WorkflowEvent, WorkflowEventStream, WorkflowExecutionMode, WorkflowInterrupt, WorkflowNode,
    WorkflowObserver, WorkflowRunResult, WorkflowRunner, DEFAULT_NODE_INVOKE_MAX_CHUNKS, END,
    START,
};
pub use error::AiError;
pub use model::{AiEventStream, ChatModel, MockChatModel};
pub use openai_compatible::OpenAiCompatibleModel;
pub use prompt::PromptTemplate;
pub use rag::{
    CharacterTextSplitter, Document, Embedder, Embedding, Indexer, RagOptions, RagOutput,
    RagPipeline, RetrievalQuery, Retriever, RozeSearchIndexer, RozeSearchRetriever, TextSplitter,
};
pub use roze_storage::ObjectStorage;
pub use runtime::AiRuntime;
pub use schema::{
    AiEvent, ContentBlock, FinishReason, GenerationOptions, Message, MessageRole, ModelRequest,
    ModelResponse, ModelUsage, ToolCall, ToolDefinition, ToolEffect,
};
pub use serde_json::Value as AiValue;
pub use storage_checkpoint::ObjectStorageCheckpointStore;
pub use team::{
    AgentExecutor, AgentTask, AgentTaskOutput, AgentTeam, DelegationTool, TeamExecutionMode,
    TeamOutput,
};
pub use tool::{Tool, ToolRegistry};
