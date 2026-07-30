use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use futures_util::{future::join_all, stream, Stream, StreamExt};
use roze_context::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{tool::check_context, AiError, Tool, ToolDefinition};

pub const START: &str = "__start__";
pub const END: &str = "__end__";
pub const DEFAULT_NODE_INVOKE_MAX_CHUNKS: usize = 4_096;

pub type NodeFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, AiError>> + Send + 'a>>;
pub type NodeStream<'a> = Pin<Box<dyn Stream<Item = Result<Value, AiError>> + Send + 'a>>;
pub type WorkflowEventStream<'a> =
    Pin<Box<dyn Stream<Item = Result<WorkflowEvent, AiError>> + Send + 'a>>;

/// One application-owned node in a compiled AI workflow.
pub trait WorkflowNode: Send + Sync {
    fn name(&self) -> &str;

    fn invoke<'a>(&'a self, context: &'a Context, input: Value) -> NodeFuture<'a>;

    /// Streams output chunks for one input.
    ///
    /// Invoke-only nodes inherit a one-chunk compatibility stream.
    fn stream<'a>(&'a self, context: &'a Context, input: Value) -> NodeStream<'a> {
        Box::pin(stream::once(
            async move { self.invoke(context, input).await },
        ))
    }
}

/// Closure-backed workflow node.
pub struct FnNode<F> {
    name: String,
    handler: F,
}

impl<F> FnNode<F> {
    pub fn new(name: impl Into<String>, handler: F) -> Self {
        Self {
            name: name.into(),
            handler,
        }
    }
}

impl<F> WorkflowNode for FnNode<F>
where
    F: Send + Sync + for<'a> Fn(&'a Context, Value) -> NodeFuture<'a>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn invoke<'a>(&'a self, context: &'a Context, input: Value) -> NodeFuture<'a> {
        (self.handler)(context, input)
    }
}

/// Closure-backed node with native chunk streaming.
pub struct FnStreamNode<F> {
    name: String,
    handler: F,
}

impl<F> FnStreamNode<F> {
    pub fn new(name: impl Into<String>, handler: F) -> Self {
        Self {
            name: name.into(),
            handler,
        }
    }
}

impl<F> WorkflowNode for FnStreamNode<F>
where
    F: Send + Sync + for<'a> Fn(&'a Context, Value) -> NodeStream<'a>,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn invoke<'a>(&'a self, context: &'a Context, input: Value) -> NodeFuture<'a> {
        Box::pin(async move {
            let mut stream = (self.handler)(context, input);
            let mut chunks = Vec::new();
            while let Some(chunk) = stream.next().await {
                if chunks.len() >= DEFAULT_NODE_INVOKE_MAX_CHUNKS {
                    return Err(AiError::WorkflowStreamLimit {
                        max_chunks: DEFAULT_NODE_INVOKE_MAX_CHUNKS,
                    });
                }
                chunks.push(chunk?);
            }
            Ok(collapse_chunks(chunks))
        })
    }

    fn stream<'a>(&'a self, context: &'a Context, input: Value) -> NodeStream<'a> {
        (self.handler)(context, input)
    }
}

/// Built-in identity node useful as a generated extension point.
pub struct PassthroughNode {
    name: String,
}

impl PassthroughNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}

impl WorkflowNode for PassthroughNode {
    fn name(&self) -> &str {
        &self.name
    }

    fn invoke<'a>(&'a self, _context: &'a Context, input: Value) -> NodeFuture<'a> {
        Box::pin(async move { Ok(input) })
    }
}

/// Synchronous workflow lifecycle callbacks for tracing and metrics adapters.
pub trait WorkflowObserver: Send + Sync {
    fn on_node_start(&self, _context: &Context, _node: &str) {}
    fn on_node_end(&self, _context: &Context, _node: &str) {}
    fn on_node_error(&self, _context: &Context, _node: &str, _error: &AiError) {}
}

#[derive(Default)]
pub struct GraphBuilder {
    nodes: BTreeMap<String, Arc<dyn WorkflowNode>>,
    edges: Vec<(String, String)>,
    observers: Vec<Arc<dyn WorkflowObserver>>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_node<N>(mut self, node: N) -> Result<Self, AiError>
    where
        N: WorkflowNode + 'static,
    {
        self.insert_node(Arc::new(node))?;
        Ok(self)
    }

    pub fn add_node_arc(mut self, node: Arc<dyn WorkflowNode>) -> Result<Self, AiError> {
        self.insert_node(node)?;
        Ok(self)
    }

    pub fn add_edge(
        mut self,
        source: impl Into<String>,
        target: impl Into<String>,
    ) -> Result<Self, AiError> {
        let edge = (source.into(), target.into());
        if self.edges.contains(&edge) {
            return Err(AiError::InvalidWorkflow(format!(
                "duplicate edge `{} -> {}`",
                edge.0, edge.1
            )));
        }
        self.edges.push(edge);
        Ok(self)
    }

    pub fn observe(mut self, observer: Arc<dyn WorkflowObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    pub fn compile(self) -> Result<CompiledGraph, AiError> {
        validate_edges(&self.nodes, &self.edges)?;
        let layers = topological_layers(&self.nodes, &self.edges)?;
        let topology = layers.iter().flatten().cloned().collect();
        validate_reachability(&self.nodes, &self.edges)?;
        let incoming = incoming_edges(&self.nodes, &self.edges);
        Ok(CompiledGraph {
            nodes: self.nodes,
            incoming,
            topology,
            layers,
            observers: self.observers,
        })
    }

    fn insert_node(&mut self, node: Arc<dyn WorkflowNode>) -> Result<(), AiError> {
        let name = node.name().trim();
        validate_node_name(name)?;
        if self.nodes.contains_key(name) {
            return Err(AiError::InvalidWorkflow(format!("duplicate node `{name}`")));
        }
        self.nodes.insert(name.to_string(), node);
        Ok(())
    }
}

#[derive(Clone)]
pub struct CompiledGraph {
    nodes: BTreeMap<String, Arc<dyn WorkflowNode>>,
    incoming: BTreeMap<String, Vec<String>>,
    topology: Vec<String>,
    layers: Vec<Vec<String>>,
    observers: Vec<Arc<dyn WorkflowObserver>>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowExecutionMode {
    #[default]
    Sequential,
    ParallelLayers,
}

/// Deterministic lifecycle and value events emitted by a compiled workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowEvent {
    RunStarted {
        run_id: String,
        mode: WorkflowExecutionMode,
    },
    NodeStarted {
        run_id: String,
        node: String,
    },
    NodeCompleted {
        run_id: String,
        node: String,
        output: Value,
    },
    RunCompleted {
        run_id: String,
        output: Value,
    },
}

impl CompiledGraph {
    pub async fn invoke(&self, context: &Context, input: Value) -> Result<Value, AiError> {
        self.invoke_with_mode(context, input, WorkflowExecutionMode::Sequential)
            .await
    }

    pub async fn invoke_with_mode(
        &self,
        context: &Context,
        input: Value,
        mode: WorkflowExecutionMode,
    ) -> Result<Value, AiError> {
        check_context(context)?;
        let mut values = BTreeMap::from([(START.to_string(), input)]);
        match mode {
            WorkflowExecutionMode::Sequential => {
                for name in &self.topology {
                    let output = self.execute_node(context, name, &values, None).await?;
                    values.insert(name.clone(), output);
                }
            }
            WorkflowExecutionMode::ParallelLayers => {
                for layer in &self.layers {
                    check_context(context)?;
                    let executions = layer.iter().map(|name| async {
                        (
                            name.clone(),
                            self.execute_node(context, name, &values, None).await,
                        )
                    });
                    let outputs = join_all(executions).await;
                    for (name, output) in outputs {
                        values.insert(name, output?);
                    }
                }
            }
        }
        collect_inputs(END, &self.incoming, &values)
    }

    /// Streams graph execution events in deterministic topology order.
    ///
    /// Parallel layers execute concurrently, while completion events are
    /// emitted in the layer's stable node-name order.
    pub fn stream<'a>(
        &'a self,
        context: &'a Context,
        input: Value,
        mode: WorkflowExecutionMode,
    ) -> WorkflowEventStream<'a> {
        Box::pin(async_stream::try_stream! {
            check_context(context)?;
            let run_id = Uuid::now_v7().to_string();
            yield WorkflowEvent::RunStarted {
                run_id: run_id.clone(),
                mode,
            };
            let mut values = BTreeMap::from([(START.to_string(), input)]);
            match mode {
                WorkflowExecutionMode::Sequential => {
                    for name in &self.topology {
                        yield WorkflowEvent::NodeStarted {
                            run_id: run_id.clone(),
                            node: name.clone(),
                        };
                        let output = self.execute_node(context, name, &values, None).await?;
                        values.insert(name.clone(), output.clone());
                        yield WorkflowEvent::NodeCompleted {
                            run_id: run_id.clone(),
                            node: name.clone(),
                            output,
                        };
                    }
                }
                WorkflowExecutionMode::ParallelLayers => {
                    for layer in &self.layers {
                        check_context(context)?;
                        for name in layer {
                            yield WorkflowEvent::NodeStarted {
                                run_id: run_id.clone(),
                                node: name.clone(),
                            };
                        }
                        let executions = layer.iter().map(|name| async {
                            (
                                name.clone(),
                                self.execute_node(context, name, &values, None).await,
                            )
                        });
                        for (name, output) in join_all(executions).await {
                            let output = output?;
                            values.insert(name.clone(), output.clone());
                            yield WorkflowEvent::NodeCompleted {
                                run_id: run_id.clone(),
                                node: name,
                                output,
                            };
                        }
                    }
                }
            }
            let output = collect_inputs(END, &self.incoming, &values)?;
            yield WorkflowEvent::RunCompleted { run_id, output };
        })
    }

    /// Composes node chunk streams through a strict linear workflow.
    ///
    /// Every output chunk becomes one input to the next node. Branched or
    /// joined DAGs are rejected because implicit stream merge semantics would
    /// be ambiguous; use `stream` for their deterministic execution events.
    pub fn stream_chunks<'a>(
        &'a self,
        context: &'a Context,
        input: Value,
        max_chunks: usize,
    ) -> NodeStream<'a> {
        let validation = self.validate_streaming_pipeline(max_chunks);
        let mut current: NodeStream<'a> = Box::pin(stream::once(async move {
            validation?;
            check_context(context)?;
            Ok(input)
        }));
        let emitted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        for name in &self.topology {
            let name = name.clone();
            let emitted = emitted.clone();
            current = Box::pin(
                current
                    .map(move |input| match input {
                        Ok(input) => self.execute_node_stream(
                            context,
                            name.clone(),
                            input,
                            emitted.clone(),
                            max_chunks,
                        ),
                        Err(error) => {
                            Box::pin(stream::once(async move { Err(error) })) as NodeStream<'a>
                        }
                    })
                    .flatten(),
            );
        }
        current
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn into_tool(self, definition: ToolDefinition) -> GraphTool {
        GraphTool {
            graph: self,
            definition,
        }
    }

    async fn execute_node(
        &self,
        context: &Context,
        name: &str,
        values: &BTreeMap<String, Value>,
        input_override: Option<Value>,
    ) -> Result<Value, AiError> {
        check_context(context)?;
        let input = match input_override {
            Some(input) => input,
            None => collect_inputs(name, &self.incoming, values)?,
        };
        let node = self
            .nodes
            .get(name)
            .ok_or_else(|| AiError::InvalidWorkflow(format!("missing node `{name}`")))?;
        for observer in &self.observers {
            observer.on_node_start(context, name);
        }
        match node.invoke(context, input).await {
            Ok(output) => {
                check_context(context)?;
                for observer in &self.observers {
                    observer.on_node_end(context, name);
                }
                Ok(output)
            }
            Err(error) => {
                for observer in &self.observers {
                    observer.on_node_error(context, name, &error);
                }
                Err(error)
            }
        }
    }

    fn execute_node_stream<'a>(
        &'a self,
        context: &'a Context,
        name: String,
        input: Value,
        emitted: Arc<std::sync::atomic::AtomicUsize>,
        max_chunks: usize,
    ) -> NodeStream<'a> {
        Box::pin(async_stream::try_stream! {
            check_context(context)?;
            let node = self
                .nodes
                .get(&name)
                .ok_or_else(|| AiError::InvalidWorkflow(format!("missing node `{name}`")))?;
            for observer in &self.observers {
                observer.on_node_start(context, &name);
            }
            let mut chunks = node.stream(context, input);
            while let Some(chunk) = chunks.next().await {
                let chunk = match chunk {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        for observer in &self.observers {
                            observer.on_node_error(context, &name, &error);
                        }
                        Err(error)?
                    }
                };
                check_context(context)?;
                let count = emitted.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if count >= max_chunks {
                    let error = AiError::WorkflowStreamLimit { max_chunks };
                    for observer in &self.observers {
                        observer.on_node_error(context, &name, &error);
                    }
                    Err(error)?;
                }
                yield chunk;
            }
            for observer in &self.observers {
                observer.on_node_end(context, &name);
            }
        })
    }

    fn validate_streaming_pipeline(&self, max_chunks: usize) -> Result<(), AiError> {
        if max_chunks == 0 {
            return Err(AiError::InvalidWorkflow(
                "workflow stream max_chunks must be greater than zero".to_string(),
            ));
        }
        for (target, sources) in &self.incoming {
            if sources.len() != 1 {
                return Err(AiError::InvalidWorkflow(format!(
                    "chunk streaming requires one input for `{target}`, found {}",
                    sources.len()
                )));
            }
        }
        let mut outgoing = BTreeMap::<&str, usize>::new();
        for sources in self.incoming.values() {
            for source in sources {
                *outgoing.entry(source).or_default() += 1;
            }
        }
        if outgoing.get(START).copied() != Some(1) {
            return Err(AiError::InvalidWorkflow(
                "chunk streaming requires one path from START".to_string(),
            ));
        }
        for name in &self.topology {
            if outgoing.get(name.as_str()).copied() != Some(1) {
                return Err(AiError::InvalidWorkflow(format!(
                    "chunk streaming requires one output path from `{name}`"
                )));
            }
        }
        Ok(())
    }
}

pub struct GraphTool {
    graph: CompiledGraph,
    definition: ToolDefinition,
}

#[async_trait]
impl Tool for GraphTool {
    fn definition(&self) -> ToolDefinition {
        self.definition.clone()
    }

    async fn invoke(&self, context: &Context, arguments: Value) -> Result<Value, AiError> {
        self.graph.invoke(context, arguments).await
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowCheckpoint {
    pub(crate) version: u32,
    pub(crate) run_id: String,
    pub(crate) graph_revision: String,
    pub(crate) tenant: Option<String>,
    pub(crate) subject: Option<String>,
    pub(crate) next_node_index: usize,
    pub(crate) values: BTreeMap<String, Value>,
    pub(crate) interrupted_before: Option<String>,
}

impl WorkflowCheckpoint {
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn graph_revision(&self) -> &str {
        &self.graph_revision
    }

    pub fn next_node_index(&self) -> usize {
        self.next_node_index
    }

    pub fn interrupted_before(&self) -> Option<&str> {
        self.interrupted_before.as_deref()
    }
}

#[async_trait]
pub trait CheckpointStore: Send + Sync {
    async fn load(
        &self,
        context: &Context,
        run_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>, AiError>;

    async fn save(&self, context: &Context, checkpoint: WorkflowCheckpoint) -> Result<(), AiError>;

    async fn delete(&self, context: &Context, run_id: &str) -> Result<(), AiError>;
}

#[derive(Default)]
pub struct MemoryCheckpointStore {
    checkpoints: RwLock<BTreeMap<String, WorkflowCheckpoint>>,
}

impl MemoryCheckpointStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.checkpoints
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[async_trait]
impl CheckpointStore for MemoryCheckpointStore {
    async fn load(
        &self,
        _context: &Context,
        run_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>, AiError> {
        Ok(self
            .checkpoints
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(run_id)
            .cloned())
    }

    async fn save(
        &self,
        _context: &Context,
        checkpoint: WorkflowCheckpoint,
    ) -> Result<(), AiError> {
        self.checkpoints
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(checkpoint.run_id.clone(), checkpoint);
        Ok(())
    }

    async fn delete(&self, _context: &Context, run_id: &str) -> Result<(), AiError> {
        self.checkpoints
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(run_id);
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowInterrupt {
    pub run_id: String,
    pub before_node: String,
    pub graph_revision: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorkflowRunResult {
    Completed { run_id: String, output: Value },
    Interrupted { interrupt: WorkflowInterrupt },
}

#[derive(Clone)]
pub struct WorkflowRunner {
    graph: CompiledGraph,
    store: Arc<dyn CheckpointStore>,
    graph_revision: String,
    interrupt_before: BTreeSet<String>,
}

impl WorkflowRunner {
    pub fn new(
        graph: CompiledGraph,
        store: Arc<dyn CheckpointStore>,
        graph_revision: impl Into<String>,
        interrupt_before: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, AiError> {
        let graph_revision = graph_revision.into();
        if graph_revision.trim().is_empty() {
            return Err(AiError::InvalidWorkflow(
                "workflow graph revision cannot be empty".to_string(),
            ));
        }
        let interrupt_before = interrupt_before
            .into_iter()
            .map(Into::into)
            .collect::<BTreeSet<_>>();
        for name in &interrupt_before {
            if !graph.nodes.contains_key(name) {
                return Err(AiError::InvalidWorkflow(format!(
                    "interrupt references missing workflow node `{name}`"
                )));
            }
        }
        Ok(Self {
            graph,
            store,
            graph_revision,
            interrupt_before,
        })
    }

    pub async fn start(
        &self,
        context: &Context,
        input: Value,
    ) -> Result<WorkflowRunResult, AiError> {
        check_context(context)?;
        let checkpoint = WorkflowCheckpoint {
            version: 1,
            run_id: Uuid::now_v7().to_string(),
            graph_revision: self.graph_revision.clone(),
            tenant: context.tenant(),
            subject: context.subject(),
            next_node_index: 0,
            values: BTreeMap::from([(START.to_string(), input)]),
            interrupted_before: None,
        };
        self.store.save(context, checkpoint.clone()).await?;
        self.drive(context, checkpoint, None).await
    }

    pub async fn resume(
        &self,
        context: &Context,
        run_id: &str,
        resume_input: Option<Value>,
    ) -> Result<WorkflowRunResult, AiError> {
        check_context(context)?;
        let checkpoint = self
            .store
            .load(context, run_id)
            .await?
            .ok_or_else(|| AiError::CheckpointNotFound(run_id.to_string()))?;
        if checkpoint.version != 1 {
            return Err(AiError::Checkpoint(format!(
                "unsupported checkpoint version {}",
                checkpoint.version
            )));
        }
        if checkpoint.graph_revision != self.graph_revision {
            return Err(AiError::CheckpointRevisionMismatch {
                expected: self.graph_revision.clone(),
                actual: checkpoint.graph_revision,
            });
        }
        if checkpoint.tenant != context.tenant() || checkpoint.subject != context.subject() {
            return Err(AiError::CheckpointScopeMismatch);
        }
        self.drive(context, checkpoint, resume_input).await
    }

    async fn drive(
        &self,
        context: &Context,
        mut checkpoint: WorkflowCheckpoint,
        mut resume_input: Option<Value>,
    ) -> Result<WorkflowRunResult, AiError> {
        let approved_node = checkpoint.interrupted_before.take();
        while checkpoint.next_node_index < self.graph.topology.len() {
            check_context(context)?;
            let name = &self.graph.topology[checkpoint.next_node_index];
            if self.interrupt_before.contains(name)
                && approved_node.as_deref() != Some(name.as_str())
            {
                checkpoint.interrupted_before = Some(name.clone());
                self.store.save(context, checkpoint.clone()).await?;
                return Ok(WorkflowRunResult::Interrupted {
                    interrupt: WorkflowInterrupt {
                        run_id: checkpoint.run_id,
                        before_node: name.clone(),
                        graph_revision: self.graph_revision.clone(),
                    },
                });
            }
            let input_override = if approved_node.as_deref() == Some(name.as_str()) {
                resume_input.take()
            } else {
                None
            };
            let output = self
                .graph
                .execute_node(context, name, &checkpoint.values, input_override)
                .await?;
            checkpoint.values.insert(name.clone(), output);
            checkpoint.next_node_index += 1;
            checkpoint.interrupted_before = None;
            self.store.save(context, checkpoint.clone()).await?;
        }
        let output = collect_inputs(END, &self.graph.incoming, &checkpoint.values)?;
        self.store.delete(context, &checkpoint.run_id).await?;
        Ok(WorkflowRunResult::Completed {
            run_id: checkpoint.run_id,
            output,
        })
    }
}

fn validate_node_name(name: &str) -> Result<(), AiError> {
    if name.is_empty() {
        return Err(AiError::InvalidWorkflow(
            "workflow node name cannot be empty".to_string(),
        ));
    }
    if matches!(name, START | END) {
        return Err(AiError::InvalidWorkflow(format!(
            "`{name}` is a reserved workflow node name"
        )));
    }
    Ok(())
}

fn validate_edges(
    nodes: &BTreeMap<String, Arc<dyn WorkflowNode>>,
    edges: &[(String, String)],
) -> Result<(), AiError> {
    if edges.is_empty() {
        return Err(AiError::InvalidWorkflow(
            "workflow requires at least one edge".to_string(),
        ));
    }
    for (source, target) in edges {
        if source == END {
            return Err(AiError::InvalidWorkflow(
                "workflow END cannot have outgoing edges".to_string(),
            ));
        }
        if target == START {
            return Err(AiError::InvalidWorkflow(
                "workflow START cannot have incoming edges".to_string(),
            ));
        }
        if source != START && !nodes.contains_key(source) {
            return Err(AiError::InvalidWorkflow(format!(
                "edge references missing source node `{source}`"
            )));
        }
        if target != END && !nodes.contains_key(target) {
            return Err(AiError::InvalidWorkflow(format!(
                "edge references missing target node `{target}`"
            )));
        }
    }
    if !edges.iter().any(|(source, _)| source == START) {
        return Err(AiError::InvalidWorkflow(
            "workflow requires an edge from START".to_string(),
        ));
    }
    if !edges.iter().any(|(_, target)| target == END) {
        return Err(AiError::InvalidWorkflow(
            "workflow requires an edge to END".to_string(),
        ));
    }
    Ok(())
}

fn incoming_edges(
    nodes: &BTreeMap<String, Arc<dyn WorkflowNode>>,
    edges: &[(String, String)],
) -> BTreeMap<String, Vec<String>> {
    let mut incoming = nodes
        .keys()
        .map(|name| (name.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    incoming.insert(END.to_string(), Vec::new());
    for (source, target) in edges {
        incoming
            .entry(target.clone())
            .or_default()
            .push(source.clone());
    }
    incoming
}

fn topological_layers(
    nodes: &BTreeMap<String, Arc<dyn WorkflowNode>>,
    edges: &[(String, String)],
) -> Result<Vec<Vec<String>>, AiError> {
    let mut indegree = nodes
        .keys()
        .map(|name| (name.clone(), 0_usize))
        .collect::<BTreeMap<_, _>>();
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for (source, target) in edges {
        if target != END {
            if source != START {
                *indegree
                    .get_mut(target)
                    .expect("edge nodes validated before topology") += 1;
            }
            outgoing
                .entry(source.clone())
                .or_default()
                .push(target.clone());
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(name, degree)| (*degree == 0).then_some(name.clone()))
        .collect::<BTreeSet<_>>();
    let mut layers = Vec::new();
    let mut visited = 0_usize;
    while !ready.is_empty() {
        let layer = std::mem::take(&mut ready).into_iter().collect::<Vec<_>>();
        visited += layer.len();
        for name in &layer {
            for target in outgoing.get(name).into_iter().flatten() {
                let degree = indegree
                    .get_mut(target)
                    .expect("edge nodes validated before topology");
                *degree -= 1;
                if *degree == 0 {
                    ready.insert(target.clone());
                }
            }
        }
        layers.push(layer);
    }
    if visited != nodes.len() {
        return Err(AiError::InvalidWorkflow(
            "workflow contains a cycle".to_string(),
        ));
    }
    Ok(layers)
}

fn validate_reachability(
    nodes: &BTreeMap<String, Arc<dyn WorkflowNode>>,
    edges: &[(String, String)],
) -> Result<(), AiError> {
    let mut outgoing = BTreeMap::<String, Vec<String>>::new();
    for (source, target) in edges {
        outgoing
            .entry(source.clone())
            .or_default()
            .push(target.clone());
    }
    let mut queue = VecDeque::from([START.to_string()]);
    let mut visited = BTreeSet::new();
    while let Some(name) = queue.pop_front() {
        if !visited.insert(name.clone()) {
            continue;
        }
        queue.extend(outgoing.get(&name).into_iter().flatten().cloned());
    }
    for name in nodes.keys() {
        if !visited.contains(name) {
            return Err(AiError::InvalidWorkflow(format!(
                "workflow node `{name}` is unreachable from START"
            )));
        }
    }
    if !visited.contains(END) {
        return Err(AiError::InvalidWorkflow(
            "workflow END is unreachable from START".to_string(),
        ));
    }
    Ok(())
}

fn collect_inputs(
    name: &str,
    incoming: &BTreeMap<String, Vec<String>>,
    values: &BTreeMap<String, Value>,
) -> Result<Value, AiError> {
    let sources = incoming
        .get(name)
        .ok_or_else(|| AiError::InvalidWorkflow(format!("missing inputs for `{name}`")))?;
    let mut inputs = sources
        .iter()
        .map(|source| {
            values.get(source).cloned().ok_or_else(|| {
                AiError::InvalidWorkflow(format!(
                    "workflow value from `{source}` is unavailable for `{name}`"
                ))
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    match inputs.len() {
        0 => Err(AiError::InvalidWorkflow(format!(
            "workflow node `{name}` has no input"
        ))),
        1 => inputs.pop().ok_or_else(|| {
            AiError::InvalidWorkflow(format!("workflow node `{name}` has no input"))
        }),
        _ => Ok(Value::Array(inputs)),
    }
}

fn collapse_chunks(mut chunks: Vec<Value>) -> Value {
    match chunks.len() {
        0 => Value::Null,
        1 => chunks.pop().unwrap_or(Value::Null),
        _ => Value::Array(chunks),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;
    use serde_json::json;
    use tokio::sync::Barrier;

    fn left(_context: &Context, input: Value) -> NodeFuture<'_> {
        Box::pin(async move { Ok(json!({"left": input})) })
    }

    fn right(_context: &Context, input: Value) -> NodeFuture<'_> {
        Box::pin(async move { Ok(json!({"right": input})) })
    }

    fn split_text(_context: &Context, input: Value) -> NodeStream<'_> {
        let chunks = input
            .as_str()
            .unwrap_or_default()
            .chars()
            .map(|character| Ok(json!(character.to_string())))
            .collect::<Vec<_>>();
        Box::pin(stream::iter(chunks))
    }

    fn uppercase(_context: &Context, input: Value) -> NodeStream<'_> {
        let output = input.as_str().unwrap_or_default().to_uppercase();
        Box::pin(stream::once(async move { Ok(json!(output)) }))
    }

    fn endless_chunks(_context: &Context, _input: Value) -> NodeStream<'_> {
        Box::pin(stream::repeat(Ok(json!("chunk"))))
    }

    struct BarrierNode {
        name: String,
        barrier: Arc<Barrier>,
    }

    impl WorkflowNode for BarrierNode {
        fn name(&self) -> &str {
            &self.name
        }

        fn invoke<'a>(&'a self, _context: &'a Context, input: Value) -> NodeFuture<'a> {
            Box::pin(async move {
                self.barrier.wait().await;
                Ok(input)
            })
        }
    }

    #[tokio::test]
    async fn graph_runs_branches_and_deterministic_join() {
        let graph = GraphBuilder::new()
            .add_node(FnNode::new("left", left))
            .expect("left")
            .add_node(FnNode::new("right", right))
            .expect("right")
            .add_node(PassthroughNode::new("join"))
            .expect("join")
            .add_edge(START, "left")
            .expect("edge")
            .add_edge(START, "right")
            .expect("edge")
            .add_edge("left", "join")
            .expect("edge")
            .add_edge("right", "join")
            .expect("edge")
            .add_edge("join", END)
            .expect("edge")
            .compile()
            .expect("compile");

        let output = graph
            .invoke(&Context::background(), json!("input"))
            .await
            .expect("invoke");
        assert_eq!(output, json!([{"left": "input"}, {"right": "input"}]));
    }

    #[tokio::test]
    async fn graph_runs_independent_layer_nodes_in_parallel() {
        let barrier = Arc::new(Barrier::new(2));
        let graph = GraphBuilder::new()
            .add_node(BarrierNode {
                name: "left".to_string(),
                barrier: barrier.clone(),
            })
            .expect("left")
            .add_node(BarrierNode {
                name: "right".to_string(),
                barrier,
            })
            .expect("right")
            .add_node(PassthroughNode::new("join"))
            .expect("join")
            .add_edge(START, "left")
            .expect("edge")
            .add_edge(START, "right")
            .expect("edge")
            .add_edge("left", "join")
            .expect("edge")
            .add_edge("right", "join")
            .expect("edge")
            .add_edge("join", END)
            .expect("edge")
            .compile()
            .expect("compile");

        let output = graph
            .invoke_with_mode(
                &Context::background(),
                json!("input"),
                WorkflowExecutionMode::ParallelLayers,
            )
            .await
            .expect("invoke");
        assert_eq!(output, json!(["input", "input"]));
    }

    #[tokio::test]
    async fn graph_streams_deterministic_execution_events() {
        let graph = GraphBuilder::new()
            .add_node(PassthroughNode::new("prepare"))
            .expect("prepare")
            .add_edge(START, "prepare")
            .expect("edge")
            .add_edge("prepare", END)
            .expect("edge")
            .compile()
            .expect("compile");

        let events = graph
            .stream(
                &Context::background(),
                json!("input"),
                WorkflowExecutionMode::Sequential,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("stream");

        assert_eq!(events.len(), 4);
        assert!(matches!(events[0], WorkflowEvent::RunStarted { .. }));
        assert!(matches!(
            &events[1],
            WorkflowEvent::NodeStarted { node, .. } if node == "prepare"
        ));
        assert!(matches!(
            &events[2],
            WorkflowEvent::NodeCompleted { node, output, .. }
                if node == "prepare" && output == &json!("input")
        ));
        assert!(matches!(
            &events[3],
            WorkflowEvent::RunCompleted { output, .. } if output == &json!("input")
        ));
    }

    #[tokio::test]
    async fn parallel_graph_stream_stabilizes_layer_event_order() {
        let graph = GraphBuilder::new()
            .add_node(FnNode::new("right", right))
            .expect("right")
            .add_node(FnNode::new("left", left))
            .expect("left")
            .add_node(PassthroughNode::new("join"))
            .expect("join")
            .add_edge(START, "right")
            .expect("edge")
            .add_edge(START, "left")
            .expect("edge")
            .add_edge("right", "join")
            .expect("edge")
            .add_edge("left", "join")
            .expect("edge")
            .add_edge("join", END)
            .expect("edge")
            .compile()
            .expect("compile");

        let events = graph
            .stream(
                &Context::background(),
                json!("input"),
                WorkflowExecutionMode::ParallelLayers,
            )
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("stream");
        let nodes = events
            .iter()
            .filter_map(|event| match event {
                WorkflowEvent::NodeStarted { node, .. }
                | WorkflowEvent::NodeCompleted { node, .. } => Some(node.as_str()),
                WorkflowEvent::RunStarted { .. } | WorkflowEvent::RunCompleted { .. } => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(nodes, ["left", "right", "left", "right", "join", "join"]);
    }

    #[tokio::test]
    async fn linear_graph_composes_native_node_chunk_streams() {
        let graph = GraphBuilder::new()
            .add_node(FnStreamNode::new("split", split_text))
            .expect("split")
            .add_node(FnStreamNode::new("uppercase", uppercase))
            .expect("uppercase")
            .add_edge(START, "split")
            .expect("edge")
            .add_edge("split", "uppercase")
            .expect("edge")
            .add_edge("uppercase", END)
            .expect("edge")
            .compile()
            .expect("compile");

        let chunks = graph
            .stream_chunks(&Context::background(), json!("ab"), 4)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("chunks");
        assert_eq!(chunks, [json!("A"), json!("B")]);
    }

    #[tokio::test]
    async fn chunk_stream_rejects_branches_and_enforces_global_bound() {
        let branch = GraphBuilder::new()
            .add_node(PassthroughNode::new("left"))
            .expect("left")
            .add_node(PassthroughNode::new("right"))
            .expect("right")
            .add_edge(START, "left")
            .expect("edge")
            .add_edge(START, "right")
            .expect("edge")
            .add_edge("left", END)
            .expect("edge")
            .add_edge("right", END)
            .expect("edge")
            .compile()
            .expect("compile");
        let rejected = branch
            .stream_chunks(&Context::background(), json!("input"), 10)
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            rejected.as_slice(),
            [Err(AiError::InvalidWorkflow(_))]
        ));

        let linear = GraphBuilder::new()
            .add_node(FnStreamNode::new("split", split_text))
            .expect("split")
            .add_node(FnStreamNode::new("uppercase", uppercase))
            .expect("uppercase")
            .add_edge(START, "split")
            .expect("edge")
            .add_edge("split", "uppercase")
            .expect("edge")
            .add_edge("uppercase", END)
            .expect("edge")
            .compile()
            .expect("compile");
        let bounded = linear
            .stream_chunks(&Context::background(), json!("ab"), 3)
            .collect::<Vec<_>>()
            .await;
        assert!(matches!(
            bounded.last(),
            Some(Err(AiError::WorkflowStreamLimit { max_chunks: 3 }))
        ));
    }

    #[tokio::test]
    async fn native_stream_node_invoke_is_also_bounded() {
        let node = FnStreamNode::new("endless", endless_chunks);
        let result = node.invoke(&Context::background(), Value::Null).await;
        assert_eq!(
            result,
            Err(AiError::WorkflowStreamLimit {
                max_chunks: DEFAULT_NODE_INVOKE_MAX_CHUNKS,
            })
        );
    }

    #[tokio::test]
    async fn runner_checkpoints_interrupts_and_resumes_with_human_input() {
        let graph = GraphBuilder::new()
            .add_node(PassthroughNode::new("review"))
            .expect("review")
            .add_edge(START, "review")
            .expect("edge")
            .add_edge("review", END)
            .expect("edge")
            .compile()
            .expect("compile");
        let store = Arc::new(MemoryCheckpointStore::new());
        let runner = WorkflowRunner::new(graph, store.clone(), "v1", ["review"]).expect("runner");

        let interrupted = runner
            .start(&Context::background(), json!("draft"))
            .await
            .expect("start");
        let run_id = match interrupted {
            WorkflowRunResult::Interrupted { interrupt } => {
                assert_eq!(interrupt.before_node, "review");
                interrupt.run_id
            }
            WorkflowRunResult::Completed { .. } => panic!("expected interrupt"),
        };
        assert_eq!(store.len(), 1);

        let completed = runner
            .resume(&Context::background(), &run_id, Some(json!("approved")))
            .await
            .expect("resume");
        assert_eq!(
            completed,
            WorkflowRunResult::Completed {
                run_id,
                output: json!("approved"),
            }
        );
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn runner_rejects_checkpoint_from_another_graph_revision() {
        let graph = GraphBuilder::new()
            .add_node(PassthroughNode::new("review"))
            .expect("review")
            .add_edge(START, "review")
            .expect("edge")
            .add_edge("review", END)
            .expect("edge")
            .compile()
            .expect("compile");
        let store = Arc::new(MemoryCheckpointStore::new());
        let first =
            WorkflowRunner::new(graph.clone(), store.clone(), "v1", ["review"]).expect("runner");
        let run_id = match first
            .start(&Context::background(), json!("draft"))
            .await
            .expect("start")
        {
            WorkflowRunResult::Interrupted { interrupt } => interrupt.run_id,
            WorkflowRunResult::Completed { .. } => panic!("expected interrupt"),
        };
        let upgraded =
            WorkflowRunner::new(graph, store, "v2", ["review"]).expect("upgraded runner");
        let result = upgraded.resume(&Context::background(), &run_id, None).await;
        assert!(matches!(
            result,
            Err(AiError::CheckpointRevisionMismatch { .. })
        ));
    }

    #[test]
    fn graph_rejects_cycles_and_unreachable_nodes() {
        let cycle = GraphBuilder::new()
            .add_node(PassthroughNode::new("a"))
            .expect("a")
            .add_node(PassthroughNode::new("b"))
            .expect("b")
            .add_edge(START, "a")
            .expect("edge")
            .add_edge("a", "b")
            .expect("edge")
            .add_edge("b", "a")
            .expect("edge")
            .add_edge("b", END)
            .expect("edge")
            .compile();
        assert!(matches!(cycle, Err(AiError::InvalidWorkflow(_))));

        let unreachable = GraphBuilder::new()
            .add_node(PassthroughNode::new("a"))
            .expect("a")
            .add_node(PassthroughNode::new("orphan"))
            .expect("orphan")
            .add_edge(START, "a")
            .expect("edge")
            .add_edge("a", END)
            .expect("edge")
            .compile();
        assert!(matches!(unreachable, Err(AiError::InvalidWorkflow(_))));
    }
}
