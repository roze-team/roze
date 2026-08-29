use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context};

use super::{
    dependency_item, find_workspace_root, format_generated_rust_files, inherited_roze_dependency,
    local_crates_prefix, plan, validate_roze_dependency_sources, DependencySource, GenerateMode,
    GenerateOptions,
};

const APPLICATION_MARKER_START: &str = "// <roze:ai-module>";
const APPLICATION_MARKER_END: &str = "// </roze:ai-module>";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AiGenerationFeatures {
    pub workflow: bool,
    pub rag: bool,
    pub team: bool,
}

/// Generates an AI module inside an existing generated Roze REST/RPC project.
///
/// The project-level transaction copies the target into a staging directory,
/// validates and formats generated files there, then atomically promotes it.
pub fn generate_ai_module(name: &str, out: &Path, options: GenerateOptions) -> anyhow::Result<()> {
    generate_ai_module_with_features(name, out, options, AiGenerationFeatures::default())
}

pub fn generate_ai_module_with_features(
    name: &str,
    out: &Path,
    options: GenerateOptions,
    requested_features: AiGenerationFeatures,
) -> anyhow::Result<()> {
    validate_name(name)?;
    validate_target(out)?;

    let plan = plan::GenerationPlan::prepare_component(out)?;
    generate_ai_module_in_place(name, plan.staged(), out, options, requested_features)?;
    plan.commit()
}

fn generate_ai_module_in_place(
    name: &str,
    out: &Path,
    logical_out: &Path,
    options: GenerateOptions,
    requested_features: AiGenerationFeatures,
) -> anyhow::Result<()> {
    let ai_dir = out.join("src/ai");
    let marker_exists = application_has_marker(&out.join("src/application.rs"))?;
    if options.mode == GenerateMode::Create && (ai_dir.exists() || marker_exists) {
        bail!(
            "AI module already exists in {}; pass --update to preserve application-owned files or --force to replace them",
            logical_out.display()
        );
    }

    fs::create_dir_all(ai_dir.join("prompts"))
        .with_context(|| format!("failed to create {}", ai_dir.display()))?;
    fs::create_dir_all(out.join("config"))
        .with_context(|| format!("failed to create {}", out.join("config").display()))?;
    let features = AiGenerationFeatures {
        workflow: requested_features.workflow || ai_dir.join("workflow.rs").is_file(),
        rag: requested_features.rag || ai_dir.join("rag.rs").is_file(),
        team: requested_features.team || ai_dir.join("team.rs").is_file(),
    };
    update_manifest(out, logical_out, options.dependency_source, features)?;
    update_application_module(&out.join("src/application.rs"))?;

    let generated_files = [
        (ai_dir.join("mod.rs"), render_module(name, features)),
        (ai_dir.join("generated.rs"), render_generated_runtime()),
    ];
    for (path, content) in &generated_files {
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
    }

    write_application_file(
        &ai_dir.join("agent.rs"),
        &render_agent(),
        options.mode == GenerateMode::Force,
    )?;
    write_application_file(
        &ai_dir.join("tools.rs"),
        render_tools(),
        options.mode == GenerateMode::Force,
    )?;
    write_application_file(
        &ai_dir.join("prompts/system.md"),
        &render_system_prompt(name),
        options.mode == GenerateMode::Force,
    )?;
    write_application_file(
        &out.join("config/ai.example.yaml"),
        render_ai_config_example(),
        options.mode == GenerateMode::Force,
    )?;
    if features.workflow {
        write_application_file(
            &ai_dir.join("workflow.rs"),
            render_workflow(),
            options.mode == GenerateMode::Force,
        )?;
    }
    if features.rag {
        write_application_file(
            &ai_dir.join("rag.rs"),
            render_rag(),
            options.mode == GenerateMode::Force,
        )?;
    }
    if features.team {
        write_application_file(
            &ai_dir.join("team.rs"),
            render_team(),
            options.mode == GenerateMode::Force,
        )?;
    }

    format_generated_rust_files(
        out,
        &generated_files
            .into_iter()
            .map(|(path, _)| path)
            .collect::<Vec<PathBuf>>(),
    )
}

fn validate_target(out: &Path) -> anyhow::Result<()> {
    if !out.join("Cargo.toml").is_file()
        || !out.join("src/application.rs").is_file()
        || !out.join("src/svc/mod.rs").is_file()
    {
        bail!(
            "{} is not a generated Roze REST/RPC project; expected Cargo.toml, src/application.rs, and src/svc/mod.rs",
            out.display()
        );
    }
    Ok(())
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("AI module name cannot be empty");
    }
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("AI module name may contain only ASCII letters, digits, `-`, and `_`");
    }
    Ok(())
}

fn update_manifest(
    out: &Path,
    logical_out: &Path,
    source: DependencySource,
    features: AiGenerationFeatures,
) -> anyhow::Result<()> {
    let path = out.join("Cargo.toml");
    let content =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let dependencies = document
        .entry("dependencies")
        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
        .as_table_mut()
        .context("Cargo.toml [dependencies] must be a table")?;
    validate_roze_dependency_sources(dependencies)?;

    let local_prefix = match source {
        DependencySource::Git => None,
        DependencySource::Path => {
            let workspace = find_workspace_root(logical_out)?.ok_or_else(|| {
                anyhow::anyhow!(
                    "--roze-source path requires {} to be inside the Roze workspace",
                    logical_out.display()
                )
            })?;
            Some(local_crates_prefix(logical_out, &workspace)?)
        }
    };
    if !dependencies.contains_key("roze-ai") {
        let item = inherited_roze_dependency(dependencies, "roze-ai")?
            .unwrap_or_else(|| dependency_item("roze-ai", source, local_prefix.as_deref()));
        dependencies.insert("roze-ai", item);
    }
    if features.rag && !dependencies.contains_key("roze-search") {
        let item = inherited_roze_dependency(dependencies, "roze-search")?
            .unwrap_or_else(|| dependency_item("roze-search", source, local_prefix.as_deref()));
        dependencies.insert("roze-search", item);
    }
    fs::write(&path, document.to_string())
        .with_context(|| format!("failed to write {}", path.display()))
}

fn application_has_marker(path: &Path) -> anyhow::Result<bool> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    Ok(source.contains(APPLICATION_MARKER_START))
}

fn update_application_module(path: &Path) -> anyhow::Result<()> {
    let mut source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let has_start = source.contains(APPLICATION_MARKER_START);
    let has_end = source.contains(APPLICATION_MARKER_END);
    if has_start != has_end {
        bail!(
            "{} contains an incomplete Roze AI module marker",
            path.display()
        );
    }
    if has_start {
        return Ok(());
    }
    if source.lines().any(declares_ai_module) {
        bail!(
            "{} already declares an `ai` module outside the Roze marker; rename it or adopt the generated marker explicitly",
            path.display()
        );
    }
    if !source.ends_with('\n') {
        source.push('\n');
    }
    source.push_str(&format!(
        "\n{APPLICATION_MARKER_START}\n#[path = \"ai/mod.rs\"]\npub mod ai;\n{APPLICATION_MARKER_END}\n"
    ));
    fs::write(path, source).with_context(|| format!("failed to write {}", path.display()))
}

fn declares_ai_module(line: &str) -> bool {
    let declaration = line.trim();
    declaration == "mod ai;"
        || declaration == "pub mod ai;"
        || (declaration.starts_with("pub(") && declaration.ends_with(" mod ai;"))
}

fn write_application_file(path: &Path, content: &str, overwrite: bool) -> anyhow::Result<()> {
    if path.exists() && !overwrite {
        return Ok(());
    }
    fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))
}

fn render_module(name: &str, features: AiGenerationFeatures) -> String {
    let workflow = if features.workflow {
        "\npub mod workflow;\n"
    } else {
        ""
    };
    let rag = if features.rag { "pub mod rag;\n" } else { "" };
    let team = if features.team { "pub mod team;\n" } else { "" };
    format!(
        r#"//! Generated AI module index.
//!
//! `generated.rs` and this index are framework-owned. `agent.rs`, `tools.rs`,
//! and `prompts/**` are application-owned and preserved by `--update`.

mod generated;

pub mod agent;
pub mod tools;
{workflow}{rag}{team}

pub use generated::{{attach, runtime}};

pub const NAME: &str = {name:?};
"#
    )
}

fn render_generated_runtime() -> String {
    r#"//! Generated service-context integration for the AI module.

use std::sync::Arc;

use crate::svc::ServiceContext;
use roze_ai::{AiError, AiRuntime};

pub fn attach(ctx: &ServiceContext) -> Result<(), AiError> {
    let runtime = super::agent::build_runtime(&ctx.config)?;
    ctx.extensions.insert(runtime);
    Ok(())
}

pub fn runtime(ctx: &ServiceContext) -> Result<Arc<AiRuntime>, AiError> {
    ctx.extensions.get::<AiRuntime>().ok_or_else(|| {
        AiError::Internal(
            "AI runtime is not attached; call application::ai::attach in configure_context"
                .to_string(),
        )
    })
}
"#
    .to_string()
}

fn render_agent() -> String {
    r#"//! Application-owned AI agent composition.

use std::sync::Arc;

use roze_ai::{
    AgentOutput, AiError, AiRuntime, Message, MockChatModel,
};
use roze_config::ServiceConfig;
use roze_context::Context;

pub const SYSTEM_PROMPT: &str = include_str!("prompts/system.md");

pub fn build_runtime(config: &ServiceConfig) -> Result<AiRuntime, AiError> {
    let mut runtime = match config.ai.as_ref() {
        Some(config) => AiRuntime::from_config(config)?,
        None => AiRuntime::new("default", Arc::new(MockChatModel::default()))?,
    };
    super::tools::register(&mut runtime)?;
    Ok(runtime)
}

pub async fn invoke(
    context: &Context,
    runtime: &AiRuntime,
    prompt: impl Into<String>,
) -> Result<AgentOutput, AiError> {
    runtime
        .invoke(
            context,
            [
                Message::system(SYSTEM_PROMPT),
                Message::user(prompt.into()),
            ],
        )
        .await
}
"#
    .to_string()
}

fn render_tools() -> &'static str {
    r#"//! Application-owned AI tool registration.

use roze_ai::{AiError, AiRuntime};

pub fn register(runtime: &mut AiRuntime) -> Result<(), AiError> {
    let _ = runtime;
    Ok(())
}
"#
}

fn render_workflow() -> &'static str {
    r#"//! Application-owned deterministic AI workflow composition.

use std::sync::Arc;

use roze_ai::{
    AiError, AiValue, CheckpointStore, CompiledGraph, GraphBuilder, NodeStream,
    ObjectStorageCheckpointStore, ObjectStorage, PassthroughNode, WorkflowRunner, END, START,
};
use roze_context::Context;

pub fn build() -> Result<CompiledGraph, AiError> {
    GraphBuilder::new()
        .add_node(PassthroughNode::new("prepare"))?
        .add_edge(START, "prepare")?
        .add_edge("prepare", END)?
        .compile()
}

pub fn runner(
    store: Arc<dyn CheckpointStore>,
    revision: impl Into<String>,
    interrupt_before: impl IntoIterator<Item = impl Into<String>>,
) -> Result<WorkflowRunner, AiError> {
    WorkflowRunner::new(build()?, store, revision, interrupt_before)
}

pub fn stream_chunks<'a>(
    graph: &'a CompiledGraph,
    context: &'a Context,
    input: AiValue,
    max_chunks: usize,
) -> NodeStream<'a> {
    graph.stream_chunks(context, input, max_chunks)
}

pub fn durable_runner(
    storage: Arc<dyn ObjectStorage>,
    revision: impl Into<String>,
    interrupt_before: impl IntoIterator<Item = impl Into<String>>,
) -> Result<WorkflowRunner, AiError> {
    let store = Arc::new(ObjectStorageCheckpointStore::new(
        storage,
        "ai/checkpoints",
    )?);
    runner(store, revision, interrupt_before)
}
"#
}

fn render_rag() -> &'static str {
    r#"//! Application-owned RAG composition backed by the existing Roze search client.

use std::sync::Arc;

use roze_ai::{
    AiError, ChatModel, PromptTemplate, RagOptions, RagPipeline, RozeSearchRetriever,
};
use roze_search::SearchClient;

pub fn build(
    search: SearchClient,
    index: impl Into<String>,
    model: Arc<dyn ChatModel>,
) -> Result<RagPipeline, AiError> {
    let retriever = RozeSearchRetriever::new(search, index, ["content", "text"])?;
    RagPipeline::new(
        Arc::new(retriever),
        model,
        PromptTemplate::new("Question: {{question}}\n\nContext:\n{{context}}")?,
        RagOptions::default(),
    )
}
"#
}

fn render_team() -> &'static str {
    r#"//! Application-owned multi-agent team composition.

use roze_ai::{AgentTeam, AiError, DelegationTool};

pub fn build(max_tasks: usize) -> Result<AgentTeam, AiError> {
    let team = AgentTeam::new(max_tasks)?;
    // Register bounded application agents here:
    // team.register("research", research_agent)?;
    Ok(team)
}

pub fn delegation_tool(team: &AgentTeam) -> DelegationTool {
    team.delegation_tool(
        "delegate_to_agent",
        "Delegate one bounded task to a registered specialist agent.",
        ["ai.delegate"],
    )
}
"#
}

fn render_system_prompt(name: &str) -> String {
    format!(
        "You are the `{name}` assistant. Follow the application's authorization and data-handling policies.\n"
    )
}

fn render_ai_config_example() -> &'static str {
    r#"# Copy this section into config.yaml and keep the key in a secret reference.
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
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn create_project(root: &Path) {
        fs::create_dir_all(root.join("src/svc")).expect("create svc");
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        )
        .expect("write manifest");
        fs::write(
            root.join("src/application.rs"),
            "use crate::svc::ServiceContext;\n",
        )
        .expect("write application");
        fs::write(root.join("src/svc/mod.rs"), "pub struct ServiceContext;\n").expect("write svc");
    }

    fn cargo_path(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn generation_preserves_application_owned_files_on_update() {
        let root = temp_project("rozectl-ai-update");
        create_project(&root);
        let options = GenerateOptions::new(GenerateMode::Create, DependencySource::Git);
        generate_ai_module("assistant", &root, options).expect("create AI module");
        fs::write(root.join("src/ai/agent.rs"), "// custom agent\n").expect("customize");
        fs::write(root.join("src/ai/tools.rs"), "// custom tools\n").expect("customize");

        generate_ai_module(
            "assistant-v2",
            &root,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("update AI module");

        assert_eq!(
            fs::read_to_string(root.join("src/ai/agent.rs")).expect("agent"),
            "// custom agent\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("src/ai/tools.rs")).expect("tools"),
            "// custom tools\n"
        );
        assert!(fs::read_to_string(root.join("src/ai/mod.rs"))
            .expect("module")
            .contains("assistant-v2"));
        assert_eq!(
            fs::read_to_string(root.join("src/application.rs"))
                .expect("application")
                .matches(APPLICATION_MARKER_START)
                .count(),
            1
        );
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn generation_adds_workflow_rag_and_team_in_a_later_step() {
        let root = temp_project("rozectl-ai-features");
        create_project(&root);
        generate_ai_module(
            "assistant",
            &root,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect("create core");
        assert!(!root.join("src/ai/workflow.rs").exists());
        assert!(!root.join("src/ai/rag.rs").exists());

        generate_ai_module_with_features(
            "assistant",
            &root,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
            AiGenerationFeatures {
                workflow: true,
                rag: true,
                team: true,
            },
        )
        .expect("add workflow and RAG");
        assert!(root.join("src/ai/workflow.rs").is_file());
        assert!(root.join("src/ai/rag.rs").is_file());
        assert!(root.join("src/ai/team.rs").is_file());
        assert!(fs::read_to_string(root.join("Cargo.toml"))
            .expect("manifest")
            .contains("roze-search"));

        fs::write(root.join("src/ai/rag.rs"), "// custom RAG\n").expect("customize RAG");
        generate_ai_module(
            "assistant",
            &root,
            GenerateOptions::new(GenerateMode::Update, DependencySource::Git),
        )
        .expect("ordinary update preserves enabled features");
        assert_eq!(
            fs::read_to_string(root.join("src/ai/rag.rs")).expect("RAG"),
            "// custom RAG\n"
        );
        assert!(fs::read_to_string(root.join("src/ai/mod.rs"))
            .expect("module")
            .contains("pub mod rag;"));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn failed_generation_leaves_project_unchanged() {
        let root = temp_project("rozectl-ai-rollback");
        create_project(&root);
        let before = fs::read_to_string(root.join("Cargo.toml")).expect("manifest");

        let error = generate_ai_module(
            "invalid name",
            &root,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
        )
        .expect_err("invalid name");

        assert!(error.to_string().contains("may contain only"));
        assert_eq!(
            fs::read_to_string(root.join("Cargo.toml")).expect("manifest"),
            before
        );
        assert!(!root.join("src/ai").exists());
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    #[ignore = "compile-smoke: generates an AI module and runs cargo check"]
    fn generated_ai_module_compiles() {
        let root = temp_project("rozectl-ai-compile");
        fs::create_dir_all(root.join("src/svc")).expect("create svc");
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("repository root");
        fs::write(
            root.join("Cargo.toml"),
            format!(
                r#"[package]
name = "ai-smoke"
version = "0.1.0"
edition = "2021"

[workspace]

[dependencies]
anyhow = "1"
roze-ai = {{ path = "{}" }}
roze-config = {{ path = "{}" }}
roze-context = {{ path = "{}" }}
roze-service = {{ path = "{}" }}
roze-search = {{ path = "{}" }}
"#,
                cargo_path(&repository.join("crates/roze-ai")),
                cargo_path(&repository.join("crates/roze-config")),
                cargo_path(&repository.join("crates/roze-context")),
                cargo_path(&repository.join("crates/roze-service")),
                cargo_path(&repository.join("crates/roze-search")),
            ),
        )
        .expect("write manifest");
        fs::write(
            root.join("src/lib.rs"),
            "pub mod application;\npub mod svc;\n",
        )
        .expect("write lib");
        fs::write(
            root.join("src/application.rs"),
            r#"use crate::svc::ServiceContext;

pub async fn configure_context(ctx: ServiceContext) -> anyhow::Result<ServiceContext> {
    ai::attach(&ctx)?;
    Ok(ctx)
}
"#,
        )
        .expect("write application");
        fs::write(
            root.join("src/svc/mod.rs"),
            r#"pub struct ServiceContext {
    pub extensions: roze_service::ApplicationExtensions,
    pub config: roze_config::ServiceConfig,
}
"#,
        )
        .expect("write svc");

        generate_ai_module_with_features(
            "assistant",
            &root,
            GenerateOptions::new(GenerateMode::Create, DependencySource::Git),
            AiGenerationFeatures {
                workflow: true,
                rag: true,
                team: true,
            },
        )
        .expect("generate AI module");

        let status = std::process::Command::new("cargo")
            .arg("check")
            .current_dir(&root)
            .status()
            .expect("run cargo check");
        assert!(status.success(), "generated AI module must compile");
        fs::remove_dir_all(root).expect("cleanup");
    }
}
