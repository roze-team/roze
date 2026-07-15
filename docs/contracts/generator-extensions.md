# Generator extension contract

`rozectl` is both a CLI and a Rust library. It exposes
`GENERATOR_EXTENSION_API_VERSION` (currently `1`) and the
`GeneratorExtension` lifecycle. Extensions are registered on
`GeneratorRegistry`, receive an immutable `GeneratorCommand` snapshot, execute
in registration order before the built-in generator, and execute in reverse
order afterwards. The after callback is invoked for both success and failure.

Generated files marked `@generated` are owned by `rozectl` and may be replaced
by `--update` or `--force`. Application code owns the generated `*_ext.rs`
modules: they are created once and preserved by `--update`. Extensions must use
their own unmarked files or those application-owned modules and must not depend
on formatting or private helper functions inside generated artifacts.

The stable surface for API version 1 is:

- `GeneratorCommand::key()` and `GeneratorCommand::output_dir()`;
- `GeneratorRegistry::register()` for replacing a built-in handler;
- `GeneratorRegistry::register_extension()`;
- `GeneratorExtension::{name,before_generate,after_generate}`;
- `GeneratorExtensionContext::{api_version,command}`.

Model generation additionally exposes
`MODEL_GENERATOR_EXTENSION_API_VERSION` (currently `1`) and:

- `generate_model_project_with_extensions(...)`;
- `ModelGeneratorExtension::{name,transform,generate}`;
- `ModelGenerationGraph::{models,canonical_ent,orm}`;
- structured `ModelAnnotation { name, expression }` metadata on entities,
  fields, edges and indexes;
- `ModelExtensionFile` with explicit `Generated` or `Application` ownership.

Model transforms run in registration order before built-in rendering. The
canonical `.ent` schema is rendered again from the transformed graph, so
annotations added by an extension are visible to later extensions and in
`src/model/schema.ent`. Model extension files are emitted afterwards in the
same order. Absolute paths, parent traversal and replacement of core model
artifacts (`mod.rs`, `client.rs`, `schema.ent`) are rejected. Application-owned
files are created once and preserved by `--update`; generated extension files
are refreshed.

Registration is compile-time and deterministic. A future dynamic plugin loader
must negotiate this API version before invoking an extension; incompatible
major versions fail closed instead of partially generating a project.
