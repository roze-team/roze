# AI Runtime Contract

`roze-ai` is the provider-neutral semantic layer for AI applications built on
Roze. It is experimental in the first implementation phase and does not change
the stable generation behavior of REST, RPC, model, search, OpenAPI, SDK, or
deployment commands.

## Boundary

`roze-ai` owns:

- provider-neutral messages, tool calls, model requests/responses, usage, and
  runtime events;
- `ChatModel` and `Tool` extension traits;
- a bounded tool-calling `Agent`;
- a cloneable `AiRuntime` model/tool registry;
- compiled DAG workflows with explicit `START`/`END`, cycle and reachability
  validation, deterministic branch joins, optional parallel-layer execution,
  node observers, deterministic execution-event streaming, bounded linear
  chunk-stream composition, graph-as-tool adaptation, and resumable
  checkpoints;
- `PromptTemplate`, `Document`, `TextSplitter`, `Embedder`, `Retriever`, and
  `Indexer` component contracts plus a bounded `RagPipeline`;
- `RozeSearchRetriever` and `RozeSearchIndexer` adapters over the existing
  `roze-search` client;
- bounded sequential/parallel `AgentTeam` task coordination and
  permission-aware model-selected delegation;
- an OpenAI-compatible Chat Completions adapter with function tools,
  structured response format forwarding, token usage, and SSE text/tool-call
  assembly;
- a deterministic `MockChatModel` for generated projects and tests.

Existing Roze modules continue to own:

- request ID, trace ID, tenant, locale, deadline, cancellation, and propagated
  permissions through `roze-context`;
- HTTP/RPC adapters and error envelopes through existing protocol modules;
- configuration and secrets through `roze-config`;
- lifecycle and application attachment through `roze-service`;
- timeout, retry, breaker, rate limit, fallback, and metrics policy through
  existing governance modules;
- persistence, cache, search, object storage, MQ, jobs, transactions, and
  reporting through their existing `roze-*` crates.

AI providers must not introduce a second context, configuration loader,
permission system, retry loop, cache, storage abstraction, or service
lifecycle.

## Eino Capability Mapping

Roze implements Eino-style AI application capabilities as Rust-native Roze
contracts. It does not embed the Go Eino runtime and does not claim source or
API compatibility.

| Eino-style capability | Roze implementation | Reused Roze boundary |
| --- | --- | --- |
| Chat model and provider | `ChatModel`, `OpenAiCompatibleModel`, `MockChatModel` | `roze-config`, secret references, `roze-context` |
| Tools and tool calling | `Tool`, `ToolRegistry`, bounded `Agent` | Context permissions, deadline, cancellation, `roze-error` |
| Chain and graph composition | `WorkflowNode`, `FnNode`, `GraphBuilder`, `CompiledGraph` | Existing application lifecycle and observability adapters |
| Token/value streaming | `WorkflowNode::stream`, `FnStreamNode`, `stream_chunks` | Inbound Context and bounded execution policy |
| Graph lifecycle streaming | `CompiledGraph::stream`, `WorkflowEvent` | Existing HTTP/RPC/SSE protocol adapters |
| Graph as tool | `CompiledGraph::into_tool` | Standard Tool permission enforcement |
| Checkpoint and human review | `WorkflowRunner`, `CheckpointStore`, interrupt/resume | `roze-storage`; application-selected lock/lease |
| Prompt components | `PromptTemplate` | Application-owned prompt files |
| RAG | `RagPipeline`, splitter/retriever/indexer contracts | `roze-search` and generated search repositories |
| Multi-agent composition | `AgentTeam`, `DelegationTool` | Standard Context and Tool permission boundary |
| Code generation | `rozectl ai generate` and optional workflow/RAG/team phases | Existing transactional generator and ownership rules |

Automatic branch/join chunk merging, a universal vector-database abstraction,
and a generic distributed lock are deliberately not inferred. Applications
select those semantics and reuse the appropriate Roze search, storage,
database, cache, transaction, or coordination module.

## Application Integration

An application creates `AiRuntime` once during startup and attaches
`Arc<AiRuntime>` through the generated service's existing
`ApplicationExtensions`. Request logic passes the inbound
`&roze_context::Context` into the runtime.

Tools declare required permission strings in `ToolDefinition`. `ToolRegistry`
checks every declared permission against the inbound Roze context before
invocation and fails closed when any permission is missing.

The agent checks cancellation and deadline before and after model/tool calls.
`max_steps` is mandatory and greater than zero, preventing unbounded tool loops.
Provider-specific network retries remain a Roze governance integration concern;
the core agent never retries calls by itself.

## Workflow Composition

`GraphBuilder` accepts application-owned `WorkflowNode` implementations and
compiles them into an immutable, cloneable `CompiledGraph`. Compilation rejects
missing nodes, duplicate edges, cycles, unreachable nodes, and graphs without
an explicit path from `START` to `END`.

Each node receives the inbound `&roze_context::Context`. A branch receives the
same cloned input; a node with several incoming edges receives a JSON array in
edge declaration order. Execution checks cancellation and deadline before and
after each node. `WorkflowObserver` provides fixed start/end/error hooks for
adapters to existing Roze tracing and metrics. A compiled graph can be wrapped
as a normal permission-checked AI `Tool`.

`WorkflowExecutionMode::ParallelLayers` executes independent nodes in the same
topological layer concurrently, then commits their results in deterministic
node order. Nodes with a dependency remain ordered.

`CompiledGraph::stream` emits `run_started`, `node_started`, `node_completed`,
and `run_completed` values in deterministic topology order. Parallel layers
still execute concurrently; their completion events are stabilized before
emission.

`WorkflowNode::stream` is the chunk-level composition contract. Invoke-only
nodes inherit a compatible one-chunk stream; `FnStreamNode` supplies a native
stream implementation. `CompiledGraph::stream_chunks` applies backpressure:
each chunk emitted by one node becomes one input to the next node, and only
terminal chunks are returned to the caller. A mandatory `max_chunks` budget
counts all intermediate and terminal node emissions.

Chunk composition intentionally requires one strict path from `START` to
`END`. Branched or joined DAGs are rejected because implicit broadcast, merge,
zip, and ordering rules would otherwise change application semantics. Use
`CompiledGraph::stream` for deterministic lifecycle/value events on those
graphs, or model the merge explicitly in an application node. Invoking an
`FnStreamNode` through the non-streaming graph API collects that node's own
chunks as null, one scalar, or an array and enforces
`DEFAULT_NODE_INVOKE_MAX_CHUNKS`.

`WorkflowRunner` saves an opaque `WorkflowCheckpoint` after every completed
node. It can interrupt before configured nodes and resume with an optional
human-supplied replacement input. Resume validates checkpoint version, graph
revision, tenant, and subject before executing more work. Completed runs delete
their checkpoint.

`CheckpointStore` is the persistence extension contract.
`MemoryCheckpointStore` is for development and tests only.
`ObjectStorageCheckpointStore` reuses `roze-storage`, stores JSON beneath a
hashed tenant/subject scope, and validates object keys. Configure that storage
resource to allow `.json` and `application/json`. Sensitive workflow values
still require encrypted storage or an application encryption wrapper.
`ObjectStorage` has no compare-and-swap/lease contract, so applications that
allow concurrent resume attempts must serialize them with an existing database
lock, distributed lock, or job ownership mechanism.

## Multi-Agent Teams

`AgentTeam` registers named `AgentExecutor` implementations and runs a bounded
list of uniquely identified tasks. Sequential mode preserves explicit task
ordering. Parallel mode executes independent tasks concurrently while
returning outputs in declaration order and aggregating model usage.

Every task receives the same inbound Roze Context and therefore the same
deadline, cancellation, tenant, identity, and permission boundary. Team task
count and each nested Agent's `max_steps` are independently bounded. Dynamic
model-selected delegation is available through `AgentTeam::delegation_tool`.
It exposes only registered agent names, accepts one prompt, propagates the same
Context, and uses the standard `ToolRegistry` permission check. Registering
that tool on a supervisor Agent keeps delegation bounded by the supervisor's
`max_steps`. Delegated agents do not receive the delegation tool automatically,
so recursive agent trees require an explicit application decision.

## RAG Components

`roze-ai` defines AI semantics while reusing existing data clients:

- `CharacterTextSplitter` performs Unicode-safe bounded splitting;
- `Embedder`, `Retriever`, and `Indexer` are application/provider extension
  contracts;
- `RozeSearchRetriever` builds engine-appropriate text queries and consumes
  normalized `roze-search::SearchHit` values;
- `RozeSearchIndexer` delegates index and delete operations to
  `roze-search::SearchClient`;
- `RagPipeline` retrieves a bounded `top_k`, caps rendered context by character
  count, renders a validated prompt template, and invokes a configured
  `ChatModel`.

Ranking, filters, reranking, domain document mapping, embedding persistence,
and authorization policy remain application-owned. Retrieval and indexing
reuse the inbound Context cancellation/deadline and do not add retries, caches,
or storage implementations.

## Provider Configuration

AI provider settings live in `roze_config::ServiceConfig.ai`. The runtime does
not load files or environment variables itself. `roze-config` resolves
`${NAME}`, `env://NAME`, and `file://path` secret references before
`AiRuntime::from_config` builds one shared HTTP client per configured provider.

```yaml
ai:
  default_provider: default
  max_steps: 8
  providers:
    default:
      kind: openai_compatible
      base_url: https://api.openai.com/v1
      api_key: ${OPENAI_API_KEY}
      model: replace-with-your-model
      timeout_ms: 30000
```

Provider names must be non-empty, `default_provider` must exist, `max_steps`
must be between 1 and 64, URLs must use HTTP(S) without embedded credentials,
and timeout values must be non-zero. Debug output never includes the resolved
API key.

The adapter sends the Roze request and trace IDs as headers and honors the
remaining Roze deadline. It does not retry internally. HTTP 429 maps to the
existing Roze rate-limit error, request timeout/connection/5xx maps to service
unavailable, and other provider rejections map to an internal provider error
without copying response bodies into errors.

## Events

One bounded run emits:

1. `run_started`;
2. optional `message_delta` events followed by one `model_completed` per model
   step;
3. `tool_started` and `tool_completed` around each tool invocation;
4. `run_completed` after the final assistant message.

Events contain bounded operation metadata and structured values. Protocol
adapters must not log message bodies, tool arguments, tool outputs, secrets, or
provider error payloads by default.

## AI Module Generation

`rozectl ai generate <name> --out <project>` adds an AI module to an existing
generated REST/RPC project. It uses an independent command and transaction; the
current API, RPC, model, search, stream, OpenAPI, SDK, and deployment generation
paths and output ownership rules are unchanged.

Generated ownership:

- `src/ai/mod.rs` and `src/ai/generated.rs` are framework-owned and refreshed;
- `src/ai/agent.rs`, `src/ai/tools.rs`, and `src/ai/prompts/**` are
  application-owned and preserved by `--update`;
- optional `src/ai/workflow.rs` and `src/ai/rag.rs` are application-owned and
  generated with `--with-workflow` and `--with-rag`;
- optional `src/ai/team.rs` is application-owned and generated with
  `--with-team`;
- `config/ai.example.yaml` is application-owned and documents the provider
  section to copy into the service's preserved `config.yaml`;
- the marker-delimited module declaration appended to `src/application.rs` is
  generator-owned, while the rest of that file remains application-owned;
- the existing `Cargo.toml` is preserved except for adding `roze-ai` when the
  dependency is absent and `roze-search` when the RAG scaffold requires it.
  The workflow scaffold uses the `ObjectStorage` type re-exported by
  `roze-ai`, preventing duplicate `roze-storage` versions in generated
  projects.

Normal regeneration uses `--update`; `--force` intentionally replaces all
files under the generated AI module. Generation occurs in a staging copy and
is promoted only after manifest editing and Rust formatting succeed.
