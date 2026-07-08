use std::{
    convert::Infallible,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
};

use tower::{Layer, Service};

use crate::{
    response::IntoResponse,
    rest::{HttpResponse, IncomingRequest},
};

pub fn handle_error<L, F>(layer: L, handler: F) -> HandleErrorLayer<L, F> {
    HandleErrorLayer::new(layer, handler)
}

#[derive(Clone)]
pub struct HandleErrorLayer<L, F> {
    layer: L,
    handler: F,
}

impl<L, F> HandleErrorLayer<L, F> {
    pub fn new(layer: L, handler: F) -> Self {
        Self { layer, handler }
    }
}

impl<S, L, F> Layer<S> for HandleErrorLayer<L, F>
where
    L: Layer<S>,
    L::Service: Service<IncomingRequest>,
    F: Clone,
{
    type Service =
        HandleErrorService<L::Service, F, <L::Service as Service<IncomingRequest>>::Error>;

    fn layer(&self, inner: S) -> Self::Service {
        HandleErrorService {
            inner: self.layer.layer(inner),
            handler: self.handler.clone(),
            readiness_error: Arc::new(Mutex::new(None)),
        }
    }
}

pub struct HandleErrorService<S, F, E> {
    inner: S,
    handler: F,
    readiness_error: Arc<Mutex<Option<E>>>,
}

impl<S, F, E> Clone for HandleErrorService<S, F, E>
where
    S: Clone,
    F: Clone,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            handler: self.handler.clone(),
            readiness_error: self.readiness_error.clone(),
        }
    }
}

impl<S, F, Fut, R, E> Service<IncomingRequest> for HandleErrorService<S, F, E>
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = E> + Clone + Send + 'static,
    E: Send + 'static,
    S::Future: Send + 'static,
    F: Fn(E) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = R> + Send + 'static,
    R: IntoResponse + 'static,
{
    type Response = HttpResponse;
    type Error = Infallible;
    type Future = HandleErrorFuture<S::Future, F, Fut, E, R>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match self.inner.poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(error)) => {
                *self
                    .readiness_error
                    .lock()
                    .expect("handle-error readiness error mutex poisoned") = Some(error);
                Poll::Ready(Ok(()))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn call(&mut self, request: IncomingRequest) -> Self::Future {
        if let Some(error) = self
            .readiness_error
            .lock()
            .expect("handle-error readiness error mutex poisoned")
            .take()
        {
            let handler = self.handler.clone();
            return HandleErrorFuture::from_error(error, handler);
        }

        let future = self.inner.call(request);
        let handler = self.handler.clone();
        HandleErrorFuture::from_inner(future, handler)
    }
}

pub struct HandleErrorFuture<InnerFuture, F, Fut, E, R> {
    state: HandleErrorFutureState<InnerFuture, F, Fut>,
    _marker: PhantomData<fn(E) -> R>,
}

enum HandleErrorFutureState<InnerFuture, F, Fut> {
    Inner { future: InnerFuture, handler: F },
    Error { future: Fut },
    Done,
}

impl<InnerFuture, F, Fut, E, R> HandleErrorFuture<InnerFuture, F, Fut, E, R>
where
    F: Fn(E) -> Fut,
{
    fn from_inner(future: InnerFuture, handler: F) -> Self {
        Self {
            state: HandleErrorFutureState::Inner { future, handler },
            _marker: PhantomData,
        }
    }

    fn from_error(error: E, handler: F) -> Self {
        Self {
            state: HandleErrorFutureState::Error {
                future: handler(error),
            },
            _marker: PhantomData,
        }
    }
}

impl<InnerFuture, F, Fut, E, R> Future for HandleErrorFuture<InnerFuture, F, Fut, E, R>
where
    InnerFuture: Future<Output = Result<HttpResponse, E>>,
    F: Fn(E) -> Fut + Clone,
    Fut: Future<Output = R>,
    R: IntoResponse,
{
    type Output = Result<HttpResponse, Infallible>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = unsafe { self.get_unchecked_mut() };
        loop {
            match &mut this.state {
                HandleErrorFutureState::Inner { future, handler } => {
                    // SAFETY: `future` is pinned through `self`, and this state machine never
                    // moves the field after it is polled; it only replaces the whole state after
                    // the inner future returns `Ready`.
                    match unsafe { Pin::new_unchecked(future) }.poll(cx) {
                        Poll::Ready(Ok(response)) => {
                            this.state = HandleErrorFutureState::Done;
                            return Poll::Ready(Ok(response));
                        }
                        Poll::Ready(Err(error)) => {
                            let future = handler.clone()(error);
                            this.state = HandleErrorFutureState::Error { future };
                        }
                        Poll::Pending => return Poll::Pending,
                    }
                }
                HandleErrorFutureState::Error { future } => {
                    // SAFETY: `future` is pinned through `self`, and the error-mapping state is
                    // replaced only after the mapper future has completed.
                    let response = match unsafe { Pin::new_unchecked(future) }.poll(cx) {
                        Poll::Ready(response) => response,
                        Poll::Pending => return Poll::Pending,
                    };
                    this.state = HandleErrorFutureState::Done;
                    return Poll::Ready(Ok(response.into_response()));
                }
                HandleErrorFutureState::Done => panic!("polled HandleErrorFuture after completion"),
            }
        }
    }
}
