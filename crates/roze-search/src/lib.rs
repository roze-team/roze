use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
            SearchEngine::Meilisearch => json!([body]),
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

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
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
        let response = request.send().await?.error_for_status()?;
        Ok(response.json().await.unwrap_or_else(|_| json!({})))
    }
}
