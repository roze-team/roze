use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchEngine {
    Elasticsearch,
    Opensearch,
    Meilisearch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchConfig {
    pub engine: SearchEngine,
    pub url: String,
    pub api_key: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchFilterOperator {
    Eq,
    NotEq,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchFilter {
    pub field: String,
    pub operator: SearchFilterOperator,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchSort {
    pub field: String,
    pub direction: SearchSortDirection,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: Option<String>,
    #[serde(default)]
    pub filters: Vec<SearchFilter>,
    #[serde(default)]
    pub sort: Vec<SearchSort>,
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_search_limit")]
    pub limit: u64,
    #[serde(default)]
    pub attributes: Vec<String>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            query: None,
            filters: Vec::new(),
            sort: Vec::new(),
            offset: 0,
            limit: default_search_limit(),
            attributes: Vec::new(),
        }
    }
}

fn default_search_limit() -> u64 {
    20
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchPage<T> {
    pub hits: Vec<T>,
    pub offset: u64,
    pub limit: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexSettings {
    pub searchable_attributes: Vec<String>,
    pub filterable_attributes: Vec<String>,
    pub sortable_attributes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchTaskState {
    Enqueued,
    Processing,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchTaskResult {
    pub provider_id: Option<u64>,
    pub state: SearchTaskState,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone)]
pub struct SearchTask {
    client: SearchClient,
    provider_id: Option<u64>,
}

impl SearchTask {
    pub fn provider_id(&self) -> Option<u64> {
        self.provider_id
    }

    pub async fn wait(&self, timeout: Duration) -> anyhow::Result<SearchTaskResult> {
        let Some(provider_id) = self.provider_id else {
            return Ok(SearchTaskResult {
                provider_id: None,
                state: SearchTaskState::Succeeded,
                error_code: None,
                error_message: None,
            });
        };
        let started = Instant::now();
        loop {
            let value = self
                .client
                .request(reqwest::Method::GET, &format!("/tasks/{provider_id}"), None)
                .await?;
            let result = parse_task_result(provider_id, &value);
            match result.state {
                SearchTaskState::Succeeded => return Ok(result),
                SearchTaskState::Failed => {
                    anyhow::bail!(
                        "search task {provider_id} failed ({}): {}",
                        result.error_code.as_deref().unwrap_or("unknown"),
                        result.error_message.as_deref().unwrap_or("unknown failure")
                    );
                }
                SearchTaskState::Enqueued | SearchTaskState::Processing => {}
            }
            if started.elapsed() >= timeout {
                anyhow::bail!("search task {provider_id} timed out after {timeout:?}");
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

/// Engine-neutral search hit used by higher-level application modules.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: Option<String>,
    pub score: Option<f64>,
    pub document: Value,
}

#[derive(Clone)]
pub struct SearchClient {
    config: SearchConfig,
    http: reqwest::Client,
}

impl SearchClient {
    pub fn new(config: SearchConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    pub fn engine(&self) -> &SearchEngine {
        &self.config.engine
    }

    pub async fn health(&self) -> anyhow::Result<Value> {
        let path = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => "/_cluster/health",
            SearchEngine::Meilisearch => "/health",
        };
        self.request(reqwest::Method::GET, path, None).await
    }

    /// Ensures an index exists with the contract-declared primary key.
    pub async fn ensure_index(&self, index: &str, primary_key: &str) -> anyhow::Result<SearchTask> {
        let path = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => format!("/{index}"),
            SearchEngine::Meilisearch => format!("/indexes/{index}"),
        };
        let (status, value) = self
            .request_status(reqwest::Method::GET, &path, None)
            .await?;
        if status.is_success() {
            return Ok(self.completed_task());
        }
        if status != reqwest::StatusCode::NOT_FOUND {
            anyhow::bail!("search provider returned HTTP {status} while checking index: {value}");
        }
        let (method, path, body) = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
                (reqwest::Method::PUT, format!("/{index}"), json!({}))
            }
            SearchEngine::Meilisearch => (
                reqwest::Method::POST,
                "/indexes".to_string(),
                json!({"uid": index, "primaryKey": primary_key}),
            ),
        };
        let value = self.request(method, &path, Some(body)).await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn apply_settings(
        &self,
        index: &str,
        settings: &SearchIndexSettings,
    ) -> anyhow::Result<SearchTask> {
        let (method, path, body) = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => (
                reqwest::Method::PUT,
                format!("/{index}/_settings"),
                json!({}),
            ),
            SearchEngine::Meilisearch => (
                reqwest::Method::PATCH,
                format!("/indexes/{index}/settings"),
                json!({
                    "searchableAttributes": settings.searchable_attributes,
                    "filterableAttributes": settings.filterable_attributes,
                    "sortableAttributes": settings.sortable_attributes,
                }),
            ),
        };
        let value = self.request(method, &path, Some(body)).await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn index_document_task<T: Serialize + ?Sized>(
        &self,
        index: &str,
        id: &str,
        document: &T,
    ) -> anyhow::Result<SearchTask> {
        let value = self.index_document(index, id, document).await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn delete_document_task(&self, index: &str, id: &str) -> anyhow::Result<SearchTask> {
        let value = self.delete_document(index, id).await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn delete_all(&self, index: &str) -> anyhow::Result<SearchTask> {
        let (path, body) = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => (
                format!("/{index}/_delete_by_query"),
                Some(json!({"query": {"match_all": {}}})),
            ),
            SearchEngine::Meilisearch => (format!("/indexes/{index}/documents"), None),
        };
        let value = self.request(reqwest::Method::DELETE, &path, body).await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn delete_by_filter(
        &self,
        index: &str,
        filters: &[SearchFilter],
    ) -> anyhow::Result<SearchTask> {
        let (path, body) = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => (
                format!("/{index}/_delete_by_query"),
                json!({"query": elastic_filter_query(filters)?}),
            ),
            SearchEngine::Meilisearch => (
                format!("/indexes/{index}/documents/delete"),
                json!({"filter": meili_filters(filters)?}),
            ),
        };
        let value = self
            .request(reqwest::Method::POST, &path, Some(body))
            .await?;
        Ok(self.task_from_response(&value))
    }

    pub async fn index_document<T: Serialize + ?Sized>(
        &self,
        index: &str,
        id: &str,
        document: &T,
    ) -> anyhow::Result<Value> {
        let body = serde_json::to_value(document)?;
        let path = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
                format!("/{index}/_doc/{id}")
            }
            SearchEngine::Meilisearch => format!("/indexes/{index}/documents"),
        };
        let payload = match self.config.engine {
            SearchEngine::Meilisearch => json!([document_with_id(body, id)]),
            _ => body,
        };
        self.request(reqwest::Method::POST, &path, Some(payload))
            .await
    }

    pub async fn delete_document(&self, index: &str, id: &str) -> anyhow::Result<Value> {
        let path = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
                format!("/{index}/_doc/{id}")
            }
            SearchEngine::Meilisearch => format!("/indexes/{index}/documents/{id}"),
        };
        self.request(reqwest::Method::DELETE, &path, None).await
    }

    pub async fn search(&self, index: &str, query: Value) -> anyhow::Result<Value> {
        let path = match self.config.engine {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
                format!("/{index}/_search")
            }
            SearchEngine::Meilisearch => format!("/indexes/{index}/search"),
        };
        self.request(reqwest::Method::POST, &path, Some(query))
            .await
    }

    pub async fn search_page<T: serde::de::DeserializeOwned>(
        &self,
        index: &str,
        request: &SearchRequest,
    ) -> anyhow::Result<SearchPage<T>> {
        if request.limit == 0 {
            anyhow::bail!("search limit must be greater than zero");
        }
        let payload = build_search_payload(&self.config.engine, request)?;
        let response = self.search(index, payload).await?;
        normalize_search_page(response, request)
    }

    /// Executes a search and normalizes Elasticsearch/OpenSearch and
    /// Meilisearch response envelopes without imposing application ranking or
    /// document semantics.
    pub async fn search_documents(
        &self,
        index: &str,
        query: Value,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let response = self.search(index, query).await?;
        Ok(normalize_search_hits(&self.config.engine, response))
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let (status, value) = self.request_status(method, path, body).await?;
        if !status.is_success() {
            anyhow::bail!("search provider returned HTTP {status}: {value}");
        }
        Ok(value)
    }

    async fn request_status(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<(reqwest::StatusCode, Value)> {
        let url = format!(
            "{}/{}",
            self.config.url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );
        let mut request = self.http.request(method, url);
        if let Some(api_key) = &self.config.api_key {
            request = request
                .bearer_auth(api_key)
                .header("X-Meili-API-Key", api_key);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let value = response.json().await.unwrap_or_else(|_| json!({}));
        Ok((status, value))
    }

    fn completed_task(&self) -> SearchTask {
        SearchTask {
            client: self.clone(),
            provider_id: None,
        }
    }

    fn task_from_response(&self, value: &Value) -> SearchTask {
        SearchTask {
            client: self.clone(),
            provider_id: value
                .get("taskUid")
                .or_else(|| value.get("uid"))
                .and_then(Value::as_u64),
        }
    }
}

fn parse_task_result(provider_id: u64, value: &Value) -> SearchTaskResult {
    let state = match value.get("status").and_then(Value::as_str) {
        Some("succeeded") => SearchTaskState::Succeeded,
        Some("failed") | Some("canceled") => SearchTaskState::Failed,
        Some("processing") => SearchTaskState::Processing,
        _ => SearchTaskState::Enqueued,
    };
    let error = value.get("error");
    SearchTaskResult {
        provider_id: Some(provider_id),
        state,
        error_code: error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(str::to_string),
        error_message: error
            .and_then(|error| error.get("message"))
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn build_search_payload(engine: &SearchEngine, request: &SearchRequest) -> anyhow::Result<Value> {
    match engine {
        SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
            let query = request
                .query
                .as_deref()
                .filter(|query| !query.is_empty())
                .map(|query| json!({"multi_match": {"query": query, "fields": ["*"]}}))
                .unwrap_or_else(|| json!({"match_all": {}}));
            let filters = elastic_filter_clauses(&request.filters)?;
            let sort = request
                .sort
                .iter()
                .map(|sort| {
                    json!({(sort.field.clone()): match sort.direction {
                        SearchSortDirection::Asc => "asc",
                        SearchSortDirection::Desc => "desc",
                    }})
                })
                .collect::<Vec<_>>();
            let mut body = json!({
                "from": request.offset,
                "size": request.limit,
                "query": {"bool": {"must": [query], "filter": filters}},
                "sort": sort,
            });
            if !request.attributes.is_empty() {
                body["_source"] = json!(request.attributes);
            }
            Ok(body)
        }
        SearchEngine::Meilisearch => {
            let mut body = json!({
                "q": request.query.as_deref().unwrap_or_default(),
                "offset": request.offset,
                "limit": request.limit,
                "filter": meili_filters(&request.filters)?,
                "sort": request.sort.iter().map(|sort| format!("{}:{}", sort.field, match sort.direction {
                    SearchSortDirection::Asc => "asc",
                    SearchSortDirection::Desc => "desc",
                })).collect::<Vec<_>>(),
            });
            if !request.attributes.is_empty() {
                body["attributesToRetrieve"] = json!(request.attributes);
            }
            Ok(body)
        }
    }
}

fn elastic_filter_query(filters: &[SearchFilter]) -> anyhow::Result<Value> {
    Ok(json!({"bool": {"filter": elastic_filter_clauses(filters)?}}))
}

fn elastic_filter_clauses(filters: &[SearchFilter]) -> anyhow::Result<Vec<Value>> {
    filters
        .iter()
        .map(|filter| {
            let field = filter.field.clone();
            Ok(match filter.operator {
                SearchFilterOperator::Eq => json!({"term": {(field): filter.value.clone()}}),
                SearchFilterOperator::NotEq => {
                    json!({"bool": {"must_not": [{"term": {(field): filter.value.clone()}}]}})
                }
                SearchFilterOperator::In => json!({"terms": {(field): filter.value.clone()}}),
                SearchFilterOperator::Gt => {
                    json!({"range": {(field): {"gt": filter.value.clone()}}})
                }
                SearchFilterOperator::Gte => {
                    json!({"range": {(field): {"gte": filter.value.clone()}}})
                }
                SearchFilterOperator::Lt => {
                    json!({"range": {(field): {"lt": filter.value.clone()}}})
                }
                SearchFilterOperator::Lte => {
                    json!({"range": {(field): {"lte": filter.value.clone()}}})
                }
            })
        })
        .collect()
}

fn meili_filters(filters: &[SearchFilter]) -> anyhow::Result<Vec<String>> {
    filters
        .iter()
        .map(|filter| {
            if !filter
                .field
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
            {
                anyhow::bail!("invalid filter field `{}`", filter.field);
            }
            let operator = match filter.operator {
                SearchFilterOperator::Eq => "=",
                SearchFilterOperator::NotEq => "!=",
                SearchFilterOperator::Gt => ">",
                SearchFilterOperator::Gte => ">=",
                SearchFilterOperator::Lt => "<",
                SearchFilterOperator::Lte => "<=",
                SearchFilterOperator::In => "IN",
            };
            let encoded = serde_json::to_string(&filter.value)?;
            Ok(format!("{} {operator} {encoded}", filter.field))
        })
        .collect()
}

fn normalize_search_page<T: serde::de::DeserializeOwned>(
    response: Value,
    request: &SearchRequest,
) -> anyhow::Result<SearchPage<T>> {
    let (hits, total) = if let Some(hits) = response.pointer("/hits/hits").and_then(Value::as_array)
    {
        let total = response
            .pointer("/hits/total/value")
            .or_else(|| response.pointer("/hits/total"))
            .and_then(Value::as_u64)
            .unwrap_or(hits.len() as u64);
        (
            hits.iter()
                .filter_map(|hit| hit.get("_source"))
                .cloned()
                .collect::<Vec<_>>(),
            total,
        )
    } else {
        let hits = response
            .get("hits")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let total = response
            .get("totalHits")
            .or_else(|| response.get("estimatedTotalHits"))
            .and_then(Value::as_u64)
            .unwrap_or(hits.len() as u64);
        (hits, total)
    };
    Ok(SearchPage {
        hits: hits
            .into_iter()
            .map(serde_json::from_value)
            .collect::<Result<Vec<_>, _>>()?,
        offset: request.offset,
        limit: request.limit,
        total,
    })
}

fn normalize_search_hits(engine: &SearchEngine, response: Value) -> Vec<SearchHit> {
    match engine {
        SearchEngine::Elasticsearch | SearchEngine::Opensearch => response
            .pointer("/hits/hits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|hit| {
                let document = hit.get("_source")?.clone();
                Some(SearchHit {
                    id: hit.get("_id").and_then(Value::as_str).map(str::to_string),
                    score: hit.get("_score").and_then(Value::as_f64),
                    document,
                })
            })
            .collect(),
        SearchEngine::Meilisearch => response
            .get("hits")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .cloned()
            .map(|document| SearchHit {
                id: document.get("id").and_then(|value| match value {
                    Value::String(value) => Some(value.clone()),
                    Value::Number(value) => Some(value.to_string()),
                    _ => None,
                }),
                score: document.get("_rankingScore").and_then(Value::as_f64),
                document,
            })
            .collect(),
    }
}

fn document_with_id(mut document: Value, id: &str) -> Value {
    if let Value::Object(fields) = &mut document {
        fields.insert("id".to_string(), Value::String(id.to_string()));
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, net::SocketAddr};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[tokio::test]
    async fn elasticsearch_health_uses_cluster_health_endpoint() {
        let server = TestServer::spawn(json!({"status": "green"})).await;
        let client = SearchClient::new(SearchConfig {
            engine: SearchEngine::Elasticsearch,
            url: server.url(),
            api_key: None,
        });

        let response = client.health().await.expect("health");
        let request = server.request().await;

        assert_eq!(response["status"], "green");
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, "/_cluster/health");
    }

    #[tokio::test]
    async fn meilisearch_index_document_sends_batch_with_id_and_api_key() {
        let server = TestServer::spawn(json!({"taskUid": 1})).await;
        let client = SearchClient::new(SearchConfig {
            engine: SearchEngine::Meilisearch,
            url: server.url(),
            api_key: Some("secret".to_string()),
        });

        let response = client
            .index_document("users", "user-1", &json!({"name": "Ada"}))
            .await
            .expect("index document");
        let request = server.request().await;

        assert_eq!(response["taskUid"], json!(1));
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/indexes/users/documents");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer secret")
        );
        assert_eq!(
            request.headers.get("x-meili-api-key").map(String::as_str),
            Some("secret")
        );
        assert_eq!(
            request.body_json(),
            json!([{"id": "user-1", "name": "Ada"}])
        );
    }

    #[test]
    fn normalizes_elasticsearch_and_meilisearch_hits() {
        let elastic = normalize_search_hits(
            &SearchEngine::Elasticsearch,
            json!({
                "hits": {
                    "hits": [
                        {"_id": "doc-1", "_score": 0.75, "_source": {"text": "hello"}}
                    ]
                }
            }),
        );
        assert_eq!(
            elastic,
            vec![SearchHit {
                id: Some("doc-1".to_string()),
                score: Some(0.75),
                document: json!({"text": "hello"}),
            }]
        );

        let meili = normalize_search_hits(
            &SearchEngine::Meilisearch,
            json!({"hits": [{"id": 7, "text": "hello", "_rankingScore": 0.5}]}),
        );
        assert_eq!(meili[0].id.as_deref(), Some("7"));
        assert_eq!(meili[0].score, Some(0.5));
    }

    #[test]
    fn meilisearch_filters_escape_values_and_reject_field_injection() {
        let filters = vec![SearchFilter {
            field: "hospital_id".to_string(),
            operator: SearchFilterOperator::Eq,
            value: json!("x\" OR doctor_id = 7"),
        }];
        assert_eq!(
            meili_filters(&filters).expect("filters"),
            vec![r#"hospital_id = "x\" OR doctor_id = 7""#.to_string()]
        );

        let invalid = SearchFilter {
            field: "hospital_id OR true".to_string(),
            operator: SearchFilterOperator::Eq,
            value: json!(1),
        };
        assert!(meili_filters(&[invalid]).is_err());
    }

    #[test]
    fn typed_search_payload_contains_paging_sort_and_projection() {
        let request = SearchRequest {
            query: Some("cardiology".to_string()),
            filters: vec![SearchFilter {
                field: "hospital_id".to_string(),
                operator: SearchFilterOperator::Eq,
                value: json!(42),
            }],
            sort: vec![SearchSort {
                field: "score".to_string(),
                direction: SearchSortDirection::Desc,
            }],
            offset: 20,
            limit: 10,
            attributes: vec!["id".to_string(), "name".to_string()],
        };
        let payload = build_search_payload(&SearchEngine::Meilisearch, &request).expect("payload");
        assert_eq!(payload["offset"], 20);
        assert_eq!(payload["limit"], 10);
        assert_eq!(payload["sort"], json!(["score:desc"]));
        assert_eq!(payload["attributesToRetrieve"], json!(["id", "name"]));
    }

    #[derive(Debug)]
    struct CapturedRequest {
        method: String,
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    impl CapturedRequest {
        fn body_json(&self) -> Value {
            serde_json::from_slice(&self.body).expect("json body")
        }
    }

    struct TestServer {
        addr: SocketAddr,
        request_rx: tokio::sync::oneshot::Receiver<CapturedRequest>,
    }

    impl TestServer {
        async fn spawn(response: Value) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            let addr = listener.local_addr().expect("addr");
            let (request_tx, request_rx) = tokio::sync::oneshot::channel();

            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let request = read_request(&mut stream).await;
                let body = serde_json::to_vec(&response).expect("response json");
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream
                    .write_all(response.as_bytes())
                    .await
                    .expect("write headers");
                stream.write_all(&body).await.expect("write body");
                let _ = request_tx.send(request);
            });

            Self { addr, request_rx }
        }

        fn url(&self) -> String {
            format!("http://{}", self.addr)
        }

        async fn request(self) -> CapturedRequest {
            self.request_rx.await.expect("captured request")
        }
    }

    async fn read_request(stream: &mut tokio::net::TcpStream) -> CapturedRequest {
        let mut buffer = Vec::new();
        let mut scratch = [0_u8; 1024];
        let header_end = loop {
            let read = stream.read(&mut scratch).await.expect("read request");
            assert!(read > 0, "connection closed before headers");
            buffer.extend_from_slice(&scratch[..read]);
            if let Some(pos) = find_header_end(&buffer) {
                break pos;
            }
        };

        let headers = std::str::from_utf8(&buffer[..header_end]).expect("headers utf8");
        let mut lines = headers.split("\r\n");
        let request_line = lines.next().expect("request line");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().expect("method").to_string();
        let path = parts.next().expect("path").to_string();
        let headers = lines
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.to_ascii_lowercase(), value.trim().to_string()))
            .collect::<HashMap<_, _>>();

        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let body_start = header_end + 4;
        let mut body = buffer[body_start..].to_vec();
        while body.len() < content_length {
            let read = stream.read(&mut scratch).await.expect("read body");
            assert!(read > 0, "connection closed before body");
            body.extend_from_slice(&scratch[..read]);
        }
        body.truncate(content_length);

        CapturedRequest {
            method,
            path,
            headers,
            body,
        }
    }

    fn find_header_end(buffer: &[u8]) -> Option<usize> {
        buffer.windows(4).position(|window| window == b"\r\n\r\n")
    }
}
