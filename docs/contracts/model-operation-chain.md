# Model operation chain

`roze-orm` defines one around-chain contract for mutation hooks, query
interceptors and traversal interceptors. `OperationMiddleware` wraps an
`Operation`; middleware may transform the input, short-circuit without
calling the next handler, call it, and transform its result or error.

Registration order is stable. The first registered middleware is outermost: it
observes input first and the downstream result last. The aliases
`MutationHook`, `QueryInterceptor`, and `TraversalInterceptor` document which
generated execution path a chain belongs to without imposing different runtime
semantics.

Privacy rules implement `Policy<Context>` and return `Allow`, `Deny`, or
`Skip`. Rules run in order; allow and deny terminate evaluation. If every rule
skips, `evaluate_policy` denies by default. A rule can return its own error for
context-dependent denial or evaluation failure.

Generated repositories must execute privacy before the terminal database
operation, then execute the applicable ordered hook/interceptor chain. Reusable
application registrations belong in preserved `src/model/*_ext.rs` modules.

Generated query and mutation builders expose `policy(rules,
deny_by_default)`. It installs `PolicyMiddleware` in the same ordered chain and
evaluates the owned typed builder immediately before downstream middleware and
the database terminal. `Allow` and `Deny` terminate rule evaluation; an
all-`Skip` set calls the supplied deny factory. Builders also expose
`mixin(value)` through `OperationMixin<Self>`, allowing one application mixin
type to implement reusable transformations for multiple generated entity and
operation types.

Generated SeaORM and Toasty create, update-one, delete-one, update-many and
delete-many builders expose `hook(...)`. Hooks receive the owned typed builder,
so they can call its public setters before invoking `next.run(mutation)`, reject
or short-circuit the operation, and inspect or transform the typed result.
Repository and transaction borrows remain valid inside the chain; hooks do not
require `'static` mutation futures.

Each generated entity also exposes `<Entity>MutationHooks`, a reusable provider
covering all five mutation builders. `ModelClient::use_<entity>_mutation_hooks`
registers providers in deterministic order. SeaORM repositories returned by the
client inherit them automatically; Toasty exposes client factories such as
`<entity>_create`, `<entity>_update_one` and `<entity>_delete_many`. Provider
hooks are installed first and are therefore outermost; hooks added directly to
the returned builder run inside them. Preserved `*_ext.rs` modules can implement
the provider as schema-level hooks without modifying generated files.

Generated SeaORM and Toasty query builders expose `around(...)` for ordered
asynchronous interception of entity-loading execution. The older `intercept(...)`
builder transform remains available for synchronous predicate/order rewriting.
`first`, `only`, pagination paths and eager loaders that terminate through
`all()` retain the registered around interceptors.

Every generated ordinary, Through and composite-key edge also exposes an
asynchronous `traverse_<edge>` method. It resolves the edge key or join rows and
returns the target entity's typed query without executing its terminal. Callers
attach traversal middleware with that query's ordered `around(...)` chain;
`query_<edge>` delegates to the same traversal and then executes `first()` or
`all()`. Nullable owning edges return `Option<TargetQuery>` so an absent foreign
key remains distinguishable from an intercepted empty result.
