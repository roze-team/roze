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
