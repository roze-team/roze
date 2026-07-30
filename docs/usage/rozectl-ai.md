# Generate an AI Module

`rozectl ai generate` adds a provider-neutral AI module to an existing
generated Roze REST or RPC project. It does not change how API, RPC, model,
search, stream, OpenAPI, SDK, Docker, or Kubernetes generation works.

The generated module provides Rust-native Roze equivalents for the common
Eino model, tool, Agent, graph, streaming, checkpoint, RAG, and multi-agent
composition patterns. It is a capability mapping rather than Go Eino API or
source compatibility; see the
[AI runtime contract](../contracts/ai-runtime.md#eino-capability-mapping).

From a local Roze checkout:

```bash
cargo run -p rozectl -- ai generate assistant \
  --out services/support \
  --roze-source path
```

With an installed `rozectl`:

```bash
rozectl ai generate assistant --out services/support
```

Generate the complete composition and RAG scaffold immediately:

```bash
rozectl ai generate assistant --out services/support \
  --with-workflow --with-rag --with-team
```

Or add capabilities later without replacing existing agent/tool code:

```bash
rozectl ai generate assistant --out services/support --update \
  --with-workflow
rozectl ai generate assistant --out services/support --update \
  --with-rag
rozectl ai generate assistant --out services/support --update \
  --with-team
```

The target must already contain `Cargo.toml`, `src/application.rs`, and
`src/svc/mod.rs`. The command adds:

```text
src/ai/
  mod.rs
  generated.rs
  agent.rs
  tools.rs
  workflow.rs        # optional, application-owned
  rag.rs             # optional, application-owned
  team.rs            # optional, application-owned
  prompts/system.md
config/
  ai.example.yaml
```

`mod.rs` and `generated.rs` are framework-owned. `agent.rs`, `tools.rs`, and
`prompts/**` are application-owned. Optional `workflow.rs`, `rag.rs`, and
`team.rs` are also application-owned. Once enabled, ordinary `--update` keeps
their module declarations and preserves their contents.

To attach the generated runtime once during service startup, add this call to
the preserved `configure_context` hook:

```rust
pub async fn configure_context(
    ctx: ServiceContext,
) -> anyhow::Result<ServiceContext> {
    ai::attach(&ctx)?;
    Ok(ctx)
}
```

The runtime is then available through `application::ai::runtime(&ctx)`. Copy
the `ai` section from `config/ai.example.yaml` into the preserved
`config.yaml`, choose the model, and provide the key through a Roze secret
reference:

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

Without an `ai` section, the generated application-owned `agent.rs` retains
the deterministic Mock development mode. With the section present it builds
all configured providers through `AiRuntime::from_config`. Register
application tools in `tools.rs`; tool permission declarations are enforced
against the inbound Roze context.

`workflow.rs` starts with a valid `START -> prepare -> END` graph. Replace the
passthrough node with application-owned `WorkflowNode` or `FnNode` components.
Graphs validate before startup, propagate the inbound Context, and can be
registered as tools. `CompiledGraph::stream` exposes deterministic run/node
execution events and values. The generated `stream_chunks` helper exposes
bounded, backpressure-aware chunk composition for strict linear graphs.
Replace nodes with `FnStreamNode` implementations when intermediate token or
value chunks must flow into the next node; branches and joins must use explicit
application merge semantics. The generated `runner` helper accepts a
`CheckpointStore`, graph revision, and interrupt-before node list. Use the
memory store only for development. The generated `durable_runner` reuses an
existing `roze_ai::ObjectStorage` (the exact `roze-storage` trait used by the
AI crate) through
`ObjectStorageCheckpointStore`; configure `.json` and `application/json` in
the storage validation policy. Add an existing lock/lease mechanism if the
same run may be resumed concurrently.

`rag.rs` builds a `RagPipeline` from the existing `roze_search::SearchClient`.
Customize the index and content field mapping there. Retrieval, indexing,
filter/ranking policy, generated search repositories, and search health remain
owned by `roze-search`; the AI module only adds document/prompt/model
composition.

`team.rs` creates a bounded `AgentTeam`. Register application agents by name,
then submit explicit `AgentTask` values in sequential or parallel mode. Its
`delegation_tool` helper exposes those registered agents to a supervisor model
under the `ai.delegate` permission. Agent, task, and supervisor step bounds
remain application-visible rather than being inferred by the generator.

Regenerate framework-owned files while preserving application code:

```bash
rozectl ai generate assistant --out services/support --update
```

Use `--force` only when intentionally replacing the complete `src/ai`
scaffold. The operation is transactional: validation, manifest editing, and
formatting happen in a staging copy before the project is replaced.
