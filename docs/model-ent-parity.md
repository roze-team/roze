# Roze Model / ent Capability Parity

This document defines practical parity between `rozectl model generate` and
[ent/ent](https://github.com/ent/ent). Parsing an ent-style schema is not
sufficient: a capability is complete only when its generated API, runtime
semantics, regeneration behavior, and supported backends are tested.

## Status

- **aligned**: equivalent typed behavior is generated and compile-tested.
- **compatible**: the capability exists with a documented semantic or
  performance difference.
- **missing**: applications still require handwritten infrastructure.

## Capability matrix

| ent capability | Roze status | Acceptance requirement |
| --- | --- | --- |
| Fields, indexes, defaults and schema round trip | aligned | Canonical `.ent` and generated-crate tests |
| Custom and composite IDs | aligned | Typed lookup, update, delete, batch and edge paths |
| O2O, O2M, M2M, inverse and Through edges | aligned | Cardinality-correct traversal in both directions |
| Predicate composition | aligned | Eq/NEQ/IN/range/string/null and AND/OR/NOT |
| `HasX`, `HasXWith` and negation | aligned | Ordinary, inverse, composite and Through edges |
| Ordering, pagination, projection and uniqueness | aligned | Typed, nullability-preserving results |
| Scalar Count/Sum/Avg/Min/Max | aligned | Predicates apply before aggregation |
| Typed grouped aggregate helpers | aligned | SeaORM and Toasty execute Count/Sum/Avg/Min/Max grouping in the database while preserving typed predicates, soft-delete scope, ordering, limit/offset and nullable results; generated value/group combinations have a fixed per-model budget and Toasty uses one parameterized derived-table statement rather than preloading keys |
| Database-side `GroupBy` and custom aggregate scan | compatible | SeaORM provides bounded HAVING/two-key grouping and `into_select`; Toasty uses bounded single-statement parameterized backend-aware SQL for typed grouping and `into_query` for native custom scans; combinations beyond the generated budget remain an application extension concern |
| Create/update/delete one and many | aligned | Validation and transaction-compatible executors |
| Arithmetic mutations (`Add<Field>`) | compatible | SeaORM atomic filtered/update-many and single/composite-key update-one add/subtract supports nullable and non-null numeric fields. Toasty 0.7 omits Decimal helpers because `rust_decimal::Decimal` does not implement `toasty::stmt::Numeric`; nullable non-Decimal query/update-many operations use one parameterized `UPDATE` because `Option<T>` is not `Numeric`, and mixed set/add chains fail explicitly |
| Upsert | compatible | SeaORM atomic; Toasty requires transaction protection |
| One-edge eager loading | aligned | Ordinary and composite-key edges are bounded to two queries; Through edges are bounded to three |
| Multiple and nested eager loading | aligned | SeaORM and Toasty expose composable `all_with_<edge>_nested` loaders for arbitrary-depth recursion; ordinary, Through and composite-key edges participate in bounded pairwise loading, with generated three-level and composite-plus-ordinary compile/clippy evidence plus generated SeaORM/SQLite runtime assertions |
| Mutation hooks | aligned | SeaORM and Toasty create/update-one/delete-one/update-many/delete-many and atomic arithmetic terminals execute ordered around hooks; reusable per-entity hook providers register on `ModelClient` and are inherited before builder-local hooks, with generated SQLite and PostgreSQL/MySQL runtime fixtures |
| Query interceptors | compatible | SeaORM and Toasty typed queries expose ordered `around(...)` interception for entity-loading terminals; projection/aggregate terminals remain |
| Traversal interceptors | aligned | Every ordinary, Through and composite-key edge exposes `traverse_<edge>` returning the target typed query; its ordered `around(...)` chain can rewrite, short-circuit or transform traversal execution, with generated compile/clippy evidence on both backends and a generated SQLite runtime assertion |
| Privacy and policy rules | compatible | Ordered allow/deny/skip, deny-by-default `PolicyMiddleware`, and generated `.policy(...)` enforcement cover entity queries, create/update/delete one/many and atomic terminals; reusable client-wide registration is supplied through generator extensions/mixins rather than implicit globals |
| Mixins | aligned | `OperationMixin<I>` is reusable across generated query/mutation builder types, every builder exposes `.mixin(...)`, and generated compile/clippy smoke covers application-defined query mixins |
| Schema migration diff/plan/apply/rollback | aligned | Deterministic drift-checked plans, transactional executors, SQLite round-trip tests and CI-gated PostgreSQL/MySQL live apply/rollback evidence |
| Custom generator extensions and annotations | aligned | `rozectl` is externally depend-able as a library; versioned command and model-graph extension lifecycles are deterministic, annotations on entities/fields/edges/indexes are structured and round-trip through canonical `.ent`, and extension outputs have enforced path/ownership rules |
| GraphQL integration | out of core | Optional integration, not core model parity |
| Gremlin storage | out of scope | Roze retains its documented SQL/Mongo scope |

## Release gate

Roze must not claim full ent parity while an in-scope row is `missing`. The
interim wording is **ent-style generated model API**.

Parity requires:

1. Every in-scope row is `aligned` or has an approved `compatible` contract.
2. The non-external `rozectl` suite and both generated-crate compile tests pass.
3. PostgreSQL, MySQL and SQLite evidence covers database aggregation,
   arithmetic mutation, migrations, hooks and policy behavior.
4. `--update` preserves every documented application-owned extension point.

## Implementation order

1. Database grouped aggregation and atomic arithmetic mutations.
   - SeaORM SQL Count/Sum/Avg/Min/Max, count HAVING and two-key grouping: done with fixed per-model combination budgets.
   - SeaORM filtered atomic add/subtract for nullable and non-null numeric fields: done.
   - Toasty filtered/update-many/update-one nullable and non-null atomic add/subtract: done for supported non-Decimal numeric types; Decimal helpers are omitted on Toasty 0.7, and query/update-many nullable scope is a single parameterized statement.
   - Toasty database-side Count/Sum/Avg/Min/Max grouping: done with a fixed per-model combination budget; typed scope is aggregated in one derived-table statement.
2. Multiple/nested eager loading with bounded query counts.
   - Pairwise ordinary, Through and composite-key combinations on SeaORM and Toasty: done.
   - Arbitrary-depth recursive paths through typed loaded-node wrappers: done; generated
     Through/Through/ordinary three-level examples compile and pass clippy on both backends.
3. Hooks and interceptors.
4. Privacy/policy enforcement and reusable mixins.
5. Migration diff/plan/apply lifecycle.
   - Version/name drift validation, deterministic apply/rollback plans, transactional
     executors and SQLite round-trip evidence: done.
   - Live PostgreSQL/MySQL apply/rollback evidence: CI-gated.
6. Stable custom generator extension API and advanced schema annotations: externally
   usable `rozectl` library, versioned command/model lifecycles, structured graph metadata,
   canonical annotation round trip and generated-file ownership/path contract done.
7. Cross-backend integration evidence and final parity release gate: `scripts/model-parity-gate.sh`.
   - SQLite executes the database semantics fixture locally; PostgreSQL/MySQL CI jobs additionally execute a generated Toasty crate that asserts grouped aggregates, nullable atomic arithmetic, ordered mutation hooks and an allow policy through the generated API.
   - A successful backend gate writes `target/model-parity-evidence/<backend>.json`; CI uploads all three files, and the final release job requires both successful dependency jobs and matching passed evidence artifacts.
