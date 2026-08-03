use sea_orm::{DatabaseConnection, DbErr, EntityTrait, FromQueryResult, PaginatorTrait, Select};
use serde::{Deserialize, Serialize};
use std::{future::Future, marker::PhantomData, pin::Pin, sync::Arc};
use thiserror::Error;

/// Selects the bounded database source used by a generated read query.
/// Transaction-scoped clients override this selection with their transaction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadSource {
    #[default]
    Replica,
    Primary,
}

impl ReadSource {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Replica => "replica",
            Self::Primary => "primary",
        }
    }
}

#[cfg(test)]
mod read_source_tests {
    use super::ReadSource;

    #[test]
    fn read_source_has_bounded_observability_labels() {
        assert_eq!(ReadSource::default(), ReadSource::Replica);
        assert_eq!(ReadSource::Replica.label(), "replica");
        assert_eq!(ReadSource::Primary.label(), "primary");
    }
}

pub type OperationFuture<'a, O, E> = Pin<Box<dyn Future<Output = Result<O, E>> + Send + 'a>>;

pub trait Operation<I, O, E>: Send + Sync {
    fn call<'a>(&'a self, input: I) -> OperationFuture<'a, O, E>
    where
        I: 'a,
        O: 'a,
        E: 'a;
}

/// An ent-style around middleware. It may rewrite input, short-circuit, invoke
/// the next handler zero or more times, and transform its output or error.
pub trait OperationMiddleware<I, O, E>: Send + Sync {
    fn call<'a>(&'a self, input: I, next: OperationNext<'a, I, O, E>) -> OperationFuture<'a, O, E>
    where
        I: 'a,
        O: 'a,
        E: 'a;
}

pub struct OperationNext<'a, I, O, E> {
    middleware: &'a [Arc<dyn OperationMiddleware<I, O, E> + 'a>],
    terminal: &'a (dyn Operation<I, O, E> + 'a),
}

impl<I, O, E> Clone for OperationNext<'_, I, O, E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I, O, E> Copy for OperationNext<'_, I, O, E> {}

impl<'a, I, O, E> OperationNext<'a, I, O, E> {
    pub fn run(self, input: I) -> OperationFuture<'a, O, E>
    where
        I: Send + 'a,
        O: Send + 'a,
        E: Send + 'a,
    {
        Box::pin(async move {
            if let Some((current, remaining)) = self.middleware.split_first() {
                current
                    .call(
                        input,
                        OperationNext {
                            middleware: remaining,
                            terminal: self.terminal,
                        },
                    )
                    .await
            } else {
                self.terminal.call(input).await
            }
        })
    }
}

/// Executes an immutable ordered middleware chain. The first registered item
/// is outermost and therefore observes the request first and result last.
pub fn execute_chain<'a, I, O, E>(
    terminal: &'a (dyn Operation<I, O, E> + 'a),
    middleware: &'a [Arc<dyn OperationMiddleware<I, O, E> + 'a>],
    input: I,
) -> OperationFuture<'a, O, E>
where
    I: Send + 'a,
    O: Send + 'a,
    E: Send + 'a,
{
    OperationNext {
        middleware,
        terminal,
    }
    .run(input)
}

pub type MutationHook<M, O, E> = dyn OperationMiddleware<M, O, E>;
pub type QueryInterceptor<Q, O, E> = dyn OperationMiddleware<Q, O, E>;
pub type TraversalInterceptor<T, O, E> = dyn OperationMiddleware<T, O, E>;

/// A reusable, synchronous builder transformation. One mixin type may
/// implement this trait for any number of generated query and mutation types.
pub trait OperationMixin<I> {
    fn apply(&self, input: I) -> I;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyDecision {
    Allow,
    Deny,
    Skip,
}

pub trait Policy<C>: Send + Sync {
    type Error;

    fn evaluate(&self, context: &C) -> Result<PolicyDecision, Self::Error>;
}

/// Evaluates rules in order. Allow and deny are terminal; an all-skip policy
/// denies by default through the caller-provided error constructor.
pub fn evaluate_policy<'policy, C, E>(
    context: &C,
    rules: &[Arc<dyn Policy<C, Error = E> + 'policy>],
    deny_by_default: impl FnOnce() -> E,
) -> Result<(), E> {
    for rule in rules {
        match rule.evaluate(context)? {
            PolicyDecision::Allow => return Ok(()),
            PolicyDecision::Deny => return Err(deny_by_default()),
            PolicyDecision::Skip => {}
        }
    }
    Err(deny_by_default())
}

/// Adapts an ordered privacy policy set to the same around-chain used by
/// generated queries and mutations. Evaluation happens before `next` and an
/// all-skip rule set denies by default.
pub struct PolicyMiddleware<'a, C, E> {
    rules: Vec<Arc<dyn Policy<C, Error = E> + 'a>>,
    deny_by_default: Arc<dyn Fn() -> E + Send + Sync + 'a>,
    marker: PhantomData<fn(C)>,
}

impl<C, E> Clone for PolicyMiddleware<'_, C, E> {
    fn clone(&self) -> Self {
        Self {
            rules: self.rules.clone(),
            deny_by_default: self.deny_by_default.clone(),
            marker: PhantomData,
        }
    }
}

impl<'a, C, E> PolicyMiddleware<'a, C, E> {
    pub fn new<I, D>(rules: I, deny_by_default: D) -> Self
    where
        I: IntoIterator<Item = Arc<dyn Policy<C, Error = E> + 'a>>,
        D: Fn() -> E + Send + Sync + 'a,
    {
        Self {
            rules: rules.into_iter().collect(),
            deny_by_default: Arc::new(deny_by_default),
            marker: PhantomData,
        }
    }
}

impl<'policy, C, O, E> OperationMiddleware<C, O, E> for PolicyMiddleware<'policy, C, E>
where
    C: Send + 'policy,
    O: Send + 'policy,
    E: Send + 'policy,
{
    fn call<'a>(&'a self, input: C, next: OperationNext<'a, C, O, E>) -> OperationFuture<'a, O, E>
    where
        C: 'a,
        O: 'a,
        E: 'a,
    {
        Box::pin(async move {
            evaluate_policy(&input, &self.rules, || (self.deny_by_default)())?;
            next.run(input).await
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRequest {
    pub page: u64,
    pub page_size: u64,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 20,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortOrder {
    Asc,
    Desc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    pub field: String,
    pub order: SortOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    In,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct QueryRequest {
    pub page: PageRequest,
    pub sorts: Vec<Sort>,
    pub filters: Vec<Filter>,
    pub tenant: Option<TenantScope>,
    pub include_deleted: bool,
}

impl QueryRequest {
    pub fn new(page: PageRequest) -> Self {
        Self {
            page,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantScope {
    pub tenant_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditFields {
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub created_at_millis: Option<u64>,
    pub updated_at_millis: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoftDeleteFields {
    pub deleted: bool,
    pub deleted_at_millis: Option<u64>,
    pub deleted_by: Option<String>,
}

impl PageRequest {
    pub fn new(page: u64, page_size: u64) -> Self {
        Self {
            page: page.max(1),
            page_size: page_size.max(1),
        }
    }

    pub fn offset(self) -> u64 {
        self.page.saturating_sub(1) * self.page_size
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
    pub page: PageRequest,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: u64, page: PageRequest) -> Self {
        Self { items, total, page }
    }

    pub fn empty(page: PageRequest) -> Self {
        Self {
            items: Vec::new(),
            total: 0,
            page,
        }
    }

    pub fn map<U, F>(self, mut f: F) -> Page<U>
    where
        F: FnMut(T) -> U,
    {
        Page {
            items: self.items.into_iter().map(&mut f).collect(),
            total: self.total,
            page: self.page,
        }
    }
}

#[derive(Debug, Error)]
pub enum OrmError {
    #[error("database error: {0}")]
    Database(#[from] DbErr),
    #[error("invalid page size")]
    InvalidPageSize,
}

pub async fn paginate_select<E>(
    db: &DatabaseConnection,
    select: Select<E>,
    page: PageRequest,
) -> Result<Page<E::Model>, OrmError>
where
    E: EntityTrait,
    E::Model: FromQueryResult + Send + Sync + 'static,
{
    if page.page_size == 0 {
        return Err(OrmError::InvalidPageSize);
    }

    let paginator = select.paginate(db, page.page_size);
    let total = paginator.num_items().await?;
    let items = paginator.fetch_page(page.page.saturating_sub(1)).await?;
    Ok(Page::new(items, total, page))
}

pub trait Repository {
    type Model;

    fn page_request(&self) -> PageRequest {
        PageRequest::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn page_request_clamps_to_valid_ranges() {
        let request = PageRequest::new(0, 0);
        assert_eq!(request.page, 1);
        assert_eq!(request.page_size, 1);
        assert_eq!(request.offset(), 0);
    }

    #[test]
    fn page_maps_items() {
        let page = Page::new(vec![1, 2, 3], 3, PageRequest::default());
        let mapped = page.map(|value| value.to_string());
        assert_eq!(mapped.items, vec!["1", "2", "3"]);
    }

    #[test]
    fn query_request_carries_common_scopes() {
        let request = QueryRequest {
            page: PageRequest::new(2, 50),
            sorts: vec![Sort {
                field: "created_at".into(),
                order: SortOrder::Desc,
            }],
            filters: vec![Filter {
                field: "status".into(),
                op: FilterOp::Eq,
                value: serde_json::json!("active"),
            }],
            tenant: Some(TenantScope {
                tenant_id: "tenant-1".into(),
            }),
            include_deleted: false,
        };

        assert_eq!(request.page.offset(), 50);
        assert_eq!(request.tenant.unwrap().tenant_id, "tenant-1");
    }

    #[tokio::test]
    async fn middleware_chain_is_ordered_and_can_transform_input_and_output() {
        struct Terminal<'a>(&'a Mutex<Vec<&'static str>>);
        impl Operation<i32, i32, &'static str> for Terminal<'_> {
            fn call<'a>(&'a self, value: i32) -> OperationFuture<'a, i32, &'static str>
            where
                i32: 'a,
                &'static str: 'a,
            {
                Box::pin(async move {
                    self.0.lock().unwrap().push("terminal");
                    Ok(value * 2)
                })
            }
        }
        struct Around {
            events: Arc<Mutex<Vec<&'static str>>>,
            before: &'static str,
            after: &'static str,
            input_delta: i32,
            output_delta: i32,
        }
        impl OperationMiddleware<i32, i32, &'static str> for Around {
            fn call<'a>(
                &'a self,
                value: i32,
                next: OperationNext<'a, i32, i32, &'static str>,
            ) -> OperationFuture<'a, i32, &'static str>
            where
                i32: 'a,
                &'static str: 'a,
            {
                Box::pin(async move {
                    self.events.lock().unwrap().push(self.before);
                    let output = next.run(value + self.input_delta).await?;
                    self.events.lock().unwrap().push(self.after);
                    Ok(output + self.output_delta)
                })
            }
        }

        let events = Arc::new(Mutex::new(Vec::new()));
        let terminal = Terminal(&events);
        let middleware: Vec<Arc<dyn OperationMiddleware<i32, i32, &'static str>>> = vec![
            Arc::new(Around {
                events: events.clone(),
                before: "outer_before",
                after: "outer_after",
                input_delta: 1,
                output_delta: 10,
            }),
            Arc::new(Around {
                events: events.clone(),
                before: "inner_before",
                after: "inner_after",
                input_delta: 0,
                output_delta: 0,
            }),
        ];

        assert_eq!(execute_chain(&terminal, &middleware, 2).await.unwrap(), 16);
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                "outer_before",
                "inner_before",
                "terminal",
                "inner_after",
                "outer_after"
            ]
        );
    }

    #[derive(Clone)]
    struct StaticPolicy(PolicyDecision);

    impl Policy<()> for StaticPolicy {
        type Error = &'static str;

        fn evaluate(&self, _context: &()) -> Result<PolicyDecision, Self::Error> {
            Ok(self.0)
        }
    }

    #[test]
    fn policies_are_ordered_and_deny_by_default() {
        let skipped: Vec<Arc<dyn Policy<(), Error = &'static str>>> =
            vec![Arc::new(StaticPolicy(PolicyDecision::Skip))];
        assert_eq!(evaluate_policy(&(), &skipped, || "denied"), Err("denied"));
        let allowed: Vec<Arc<dyn Policy<(), Error = &'static str>>> = vec![
            Arc::new(StaticPolicy(PolicyDecision::Skip)),
            Arc::new(StaticPolicy(PolicyDecision::Allow)),
            Arc::new(StaticPolicy(PolicyDecision::Deny)),
        ];
        assert_eq!(evaluate_policy(&(), &allowed, || "denied"), Ok(()));
    }

    #[tokio::test]
    async fn policy_middleware_enforces_before_the_terminal() {
        struct Terminal;
        impl Operation<(), bool, &'static str> for Terminal {
            fn call<'a>(&'a self, _input: ()) -> OperationFuture<'a, bool, &'static str>
            where
                (): 'a,
                bool: 'a,
                &'static str: 'a,
            {
                Box::pin(async { Ok(true) })
            }
        }

        let denied_rules: Vec<Arc<dyn Policy<(), Error = &'static str>>> =
            vec![Arc::new(StaticPolicy(PolicyDecision::Skip))];
        let denied = PolicyMiddleware::new(denied_rules, || "denied");
        let denied_chain: Vec<Arc<dyn OperationMiddleware<(), bool, &'static str>>> =
            vec![Arc::new(denied)];
        assert_eq!(
            execute_chain(&Terminal, &denied_chain, ()).await,
            Err("denied")
        );

        let allowed_rules: Vec<Arc<dyn Policy<(), Error = &'static str>>> =
            vec![Arc::new(StaticPolicy(PolicyDecision::Allow))];
        let allowed = PolicyMiddleware::new(allowed_rules, || "denied");
        let allowed_chain: Vec<Arc<dyn OperationMiddleware<(), bool, &'static str>>> =
            vec![Arc::new(allowed)];
        assert_eq!(execute_chain(&Terminal, &allowed_chain, ()).await, Ok(true));
    }

    #[test]
    fn operation_mixins_are_reusable_builder_transforms() {
        struct AddOne;
        impl OperationMixin<i32> for AddOne {
            fn apply(&self, input: i32) -> i32 {
                input + 1
            }
        }
        assert_eq!(AddOne.apply(41), 42);
    }
}
