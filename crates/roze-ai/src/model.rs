use std::{
    collections::VecDeque,
    pin::Pin,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use futures_util::{stream, Stream};
use roze_context::Context;

use crate::{AiError, AiEvent, ModelRequest, ModelResponse};

pub type AiEventStream<'a> = Pin<Box<dyn Stream<Item = Result<AiEvent, AiError>> + Send + 'a>>;

/// Provider-neutral chat model contract.
#[async_trait]
pub trait ChatModel: Send + Sync {
    async fn invoke(
        &self,
        context: &Context,
        request: ModelRequest,
    ) -> Result<ModelResponse, AiError>;

    /// A compatibility streaming surface.
    ///
    /// Providers with native streaming should override this method. The default
    /// implementation emits one completed-model event.
    fn stream<'a>(&'a self, context: &'a Context, request: ModelRequest) -> AiEventStream<'a> {
        Box::pin(stream::once(async move {
            let response = self.invoke(context, request).await?;
            Ok(AiEvent::ModelCompleted {
                run_id: context.request_id(),
                step: 1,
                response,
            })
        }))
    }
}

/// Deterministic model implementation for generated projects and tests.
#[derive(Clone, Default)]
pub struct MockChatModel {
    responses: Arc<Mutex<VecDeque<Result<ModelResponse, AiError>>>>,
}

impl MockChatModel {
    pub fn new(responses: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(
                responses.into_iter().map(Ok).collect::<VecDeque<_>>(),
            )),
        }
    }

    pub fn from_results(
        responses: impl IntoIterator<Item = Result<ModelResponse, AiError>>,
    ) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into_iter().collect())),
        }
    }

    pub fn push(&self, response: ModelResponse) {
        self.responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push_back(Ok(response));
    }

    pub fn remaining(&self) -> usize {
        self.responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }
}

#[async_trait]
impl ChatModel for MockChatModel {
    async fn invoke(
        &self,
        _context: &Context,
        _request: ModelRequest,
    ) -> Result<ModelResponse, AiError> {
        self.responses
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or_else(|| {
                Err(AiError::Provider(
                    "mock model has no remaining response".to_string(),
                ))
            })
    }
}
