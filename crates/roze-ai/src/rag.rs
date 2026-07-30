use std::{collections::BTreeMap, sync::Arc};

use async_trait::async_trait;
use roze_context::Context;
use roze_search::{SearchClient, SearchEngine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    tool::check_context, AiError, ChatModel, GenerationOptions, Message, ModelRequest,
    ModelResponse, PromptTemplate,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

impl Document {
    pub fn new(id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            metadata: BTreeMap::new(),
            score: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalQuery {
    pub text: String,
    pub top_k: usize,
    #[serde(default)]
    pub filters: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Embedding {
    pub vector: Vec<f32>,
}

#[async_trait]
pub trait Embedder: Send + Sync {
    async fn embed(&self, context: &Context, texts: Vec<String>)
        -> Result<Vec<Embedding>, AiError>;
}

#[async_trait]
pub trait Retriever: Send + Sync {
    async fn retrieve(
        &self,
        context: &Context,
        query: RetrievalQuery,
    ) -> Result<Vec<Document>, AiError>;
}

#[async_trait]
pub trait Indexer: Send + Sync {
    async fn upsert(&self, context: &Context, documents: Vec<Document>) -> Result<(), AiError>;
    async fn delete(&self, context: &Context, ids: Vec<String>) -> Result<(), AiError>;
}

pub trait TextSplitter: Send + Sync {
    fn split(&self, document: Document) -> Result<Vec<Document>, AiError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharacterTextSplitter {
    max_chars: usize,
    overlap_chars: usize,
}

impl CharacterTextSplitter {
    pub fn new(max_chars: usize, overlap_chars: usize) -> Result<Self, AiError> {
        if max_chars == 0 {
            return Err(AiError::InvalidRequest(
                "AI text splitter max_chars must be greater than zero".to_string(),
            ));
        }
        if overlap_chars >= max_chars {
            return Err(AiError::InvalidRequest(
                "AI text splitter overlap_chars must be smaller than max_chars".to_string(),
            ));
        }
        Ok(Self {
            max_chars,
            overlap_chars,
        })
    }
}

impl TextSplitter for CharacterTextSplitter {
    fn split(&self, document: Document) -> Result<Vec<Document>, AiError> {
        let characters = document.content.chars().collect::<Vec<_>>();
        if characters.is_empty() {
            return Ok(Vec::new());
        }
        let step = self.max_chars - self.overlap_chars;
        let mut chunks = Vec::new();
        let mut start = 0_usize;
        while start < characters.len() {
            let end = (start + self.max_chars).min(characters.len());
            let mut metadata = document.metadata.clone();
            metadata.insert("source_document_id".to_string(), json!(document.id));
            metadata.insert("chunk_index".to_string(), json!(chunks.len()));
            chunks.push(Document {
                id: format!("{}:{}", document.id, chunks.len()),
                content: characters[start..end].iter().collect(),
                metadata,
                score: document.score,
            });
            if end == characters.len() {
                break;
            }
            start = start.saturating_add(step);
        }
        Ok(chunks)
    }
}

/// Retriever backed by the existing `roze-search` client.
#[derive(Clone)]
pub struct RozeSearchRetriever {
    client: SearchClient,
    index: String,
    content_fields: Vec<String>,
}

impl RozeSearchRetriever {
    pub fn new(
        client: SearchClient,
        index: impl Into<String>,
        content_fields: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<Self, AiError> {
        let index = index.into();
        if index.trim().is_empty() {
            return Err(AiError::InvalidRequest(
                "AI search retriever index cannot be empty".to_string(),
            ));
        }
        Ok(Self {
            client,
            index,
            content_fields: content_fields.into_iter().map(Into::into).collect(),
        })
    }
}

#[async_trait]
impl Retriever for RozeSearchRetriever {
    async fn retrieve(
        &self,
        context: &Context,
        query: RetrievalQuery,
    ) -> Result<Vec<Document>, AiError> {
        check_context(context)?;
        if query.top_k == 0 {
            return Err(AiError::InvalidRequest(
                "AI retrieval top_k must be greater than zero".to_string(),
            ));
        }
        let payload = match self.client.engine() {
            SearchEngine::Elasticsearch | SearchEngine::Opensearch => {
                let search = if self.content_fields.is_empty() {
                    json!({"query_string": {"query": query.text}})
                } else {
                    json!({
                        "multi_match": {
                            "query": query.text,
                            "fields": self.content_fields,
                        }
                    })
                };
                let mut payload = json!({
                    "query": search,
                    "size": query.top_k,
                });
                if !query.filters.is_null() {
                    payload["post_filter"] = query.filters;
                }
                payload
            }
            SearchEngine::Meilisearch => {
                let mut payload = json!({
                    "q": query.text,
                    "limit": query.top_k,
                });
                if !query.filters.is_null() {
                    payload["filter"] = query.filters;
                }
                payload
            }
        };
        let hits = self
            .client
            .search_documents(&self.index, payload)
            .await
            .map_err(|error| AiError::Retrieval(error.to_string()))?;
        check_context(context)?;
        hits.into_iter()
            .enumerate()
            .map(|(position, hit)| {
                let content = document_content(&hit.document, &self.content_fields);
                if content.is_empty() {
                    return Err(AiError::Retrieval(format!(
                        "search hit at position {position} has no configured text content"
                    )));
                }
                let metadata = hit
                    .document
                    .as_object()
                    .map(|fields| {
                        fields
                            .iter()
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(Document {
                    id: hit.id.unwrap_or_else(|| format!("hit-{position}")),
                    content,
                    metadata,
                    score: hit.score.map(|score| score as f32),
                })
            })
            .collect()
    }
}

/// Indexer backed by the existing `roze-search` client.
#[derive(Clone)]
pub struct RozeSearchIndexer {
    client: SearchClient,
    index: String,
}

impl RozeSearchIndexer {
    pub fn new(client: SearchClient, index: impl Into<String>) -> Result<Self, AiError> {
        let index = index.into();
        if index.trim().is_empty() {
            return Err(AiError::InvalidRequest(
                "AI search indexer index cannot be empty".to_string(),
            ));
        }
        Ok(Self { client, index })
    }
}

#[async_trait]
impl Indexer for RozeSearchIndexer {
    async fn upsert(&self, context: &Context, documents: Vec<Document>) -> Result<(), AiError> {
        for document in documents {
            check_context(context)?;
            self.client
                .index_document(&self.index, &document.id, &document)
                .await
                .map_err(|error| AiError::Indexing(error.to_string()))?;
        }
        check_context(context)
    }

    async fn delete(&self, context: &Context, ids: Vec<String>) -> Result<(), AiError> {
        for id in ids {
            check_context(context)?;
            self.client
                .delete_document(&self.index, &id)
                .await
                .map_err(|error| AiError::Indexing(error.to_string()))?;
        }
        check_context(context)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagOptions {
    pub top_k: usize,
    pub max_context_chars: usize,
    pub system_prompt: String,
    pub generation: GenerationOptions,
}

impl Default for RagOptions {
    fn default() -> Self {
        Self {
            top_k: 5,
            max_context_chars: 12_000,
            system_prompt:
                "Answer using the supplied context. Say when the context is insufficient."
                    .to_string(),
            generation: GenerationOptions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RagOutput {
    pub response: ModelResponse,
    pub documents: Vec<Document>,
}

pub struct RagPipeline {
    retriever: Arc<dyn Retriever>,
    model: Arc<dyn ChatModel>,
    template: PromptTemplate,
    options: RagOptions,
}

impl RagPipeline {
    pub fn new(
        retriever: Arc<dyn Retriever>,
        model: Arc<dyn ChatModel>,
        template: PromptTemplate,
        options: RagOptions,
    ) -> Result<Self, AiError> {
        if options.top_k == 0 {
            return Err(AiError::InvalidRequest(
                "AI RAG top_k must be greater than zero".to_string(),
            ));
        }
        if options.max_context_chars == 0 {
            return Err(AiError::InvalidRequest(
                "AI RAG max_context_chars must be greater than zero".to_string(),
            ));
        }
        Ok(Self {
            retriever,
            model,
            template,
            options,
        })
    }

    pub async fn invoke(
        &self,
        context: &Context,
        question: impl Into<String>,
    ) -> Result<RagOutput, AiError> {
        check_context(context)?;
        let question = question.into();
        if question.trim().is_empty() {
            return Err(AiError::InvalidRequest(
                "AI RAG question cannot be empty".to_string(),
            ));
        }
        let documents = self
            .retriever
            .retrieve(
                context,
                RetrievalQuery {
                    text: question.clone(),
                    top_k: self.options.top_k,
                    filters: Value::Null,
                },
            )
            .await?;
        check_context(context)?;
        let rendered_context = render_context(&documents, self.options.max_context_chars);
        let prompt = self.template.render(&BTreeMap::from([
            ("question".to_string(), question),
            ("context".to_string(), rendered_context),
        ]))?;
        let response = self
            .model
            .invoke(
                context,
                ModelRequest {
                    messages: vec![
                        Message::system(self.options.system_prompt.clone()),
                        Message::user(prompt),
                    ],
                    tools: Vec::new(),
                    options: self.options.generation.clone(),
                },
            )
            .await?;
        check_context(context)?;
        Ok(RagOutput {
            response,
            documents,
        })
    }
}

fn document_content(document: &Value, fields: &[String]) -> String {
    let Some(object) = document.as_object() else {
        return document.as_str().unwrap_or_default().to_string();
    };
    let values = if fields.is_empty() {
        object.values().collect::<Vec<_>>()
    } else {
        fields
            .iter()
            .filter_map(|field| object.get(field))
            .collect::<Vec<_>>()
    };
    values
        .into_iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_context(documents: &[Document], max_chars: usize) -> String {
    let mut output = String::new();
    for document in documents {
        let section = format!("[document:{}]\n{}\n\n", document.id, document.content);
        let remaining = max_chars.saturating_sub(output.chars().count());
        if remaining == 0 {
            break;
        }
        output.extend(section.chars().take(remaining));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MockChatModel;

    struct StaticRetriever(Vec<Document>);

    #[async_trait]
    impl Retriever for StaticRetriever {
        async fn retrieve(
            &self,
            _context: &Context,
            _query: RetrievalQuery,
        ) -> Result<Vec<Document>, AiError> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn splitter_preserves_unicode_and_overlap() {
        let splitter = CharacterTextSplitter::new(4, 1).expect("splitter");
        let chunks = splitter
            .split(Document::new("doc", "你好世界Roze"))
            .expect("split");
        assert_eq!(chunks[0].content, "你好世界");
        assert_eq!(chunks[1].content, "界Roz");
        assert_eq!(chunks[2].content, "ze");
    }

    #[tokio::test]
    async fn rag_pipeline_retrieves_context_and_invokes_model() {
        let model = MockChatModel::new([ModelResponse::text("Roze is a framework.")]);
        let pipeline = RagPipeline::new(
            Arc::new(StaticRetriever(vec![Document::new(
                "roze",
                "Roze is a Rust microservice framework.",
            )])),
            Arc::new(model),
            PromptTemplate::new("Question: {{question}}\nContext:\n{{context}}").expect("template"),
            RagOptions::default(),
        )
        .expect("pipeline");

        let output = pipeline
            .invoke(&Context::background(), "What is Roze?")
            .await
            .expect("invoke");
        assert_eq!(output.documents.len(), 1);
        assert_eq!(
            output.response.message,
            Message::assistant("Roze is a framework.")
        );
    }
}
