use std::convert::Infallible;

use tower::Service;

use crate::{
    handler::Handler,
    rest::{HttpResponse, IncomingRequest},
};

use super::{MethodFilter, MethodRouter};

macro_rules! chained_handler_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Chain a handler for `", stringify!($method), "` requests.")]
        #[track_caller]
        pub fn $name<H, T>(self, handler: H) -> Self
        where
            H: crate::handler::Handler<T>,
        {
            self.on(http::Method::$method, handler)
        }
    };
}

macro_rules! chained_service_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Chain a service for `", stringify!($method), "` requests.")]
        pub fn $name<S>(self, service: S) -> Self
        where
            S: tower::Service<
                    crate::rest::IncomingRequest,
                    Response = crate::rest::HttpResponse,
                    Error = std::convert::Infallible,
                > + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            self.on_service(http::Method::$method, service)
        }
    };
}

pub(super) use {chained_handler_fn, chained_service_fn};

macro_rules! top_level_handler_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Route `", stringify!($method), "` requests to the handler.")]
        #[track_caller]
        pub fn $name<H, T>(handler: H) -> MethodRouter
        where
            H: Handler<T>,
        {
            on(MethodFilter::$method, handler)
        }
    };
}

macro_rules! top_level_service_fn {
    ($name:ident, $method:ident) => {
        #[doc = concat!("Route `", stringify!($method), "` requests to the service.")]
        #[track_caller]
        pub fn $name<S>(service: S) -> MethodRouter
        where
            S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
                + Clone
                + Send
                + Sync
                + 'static,
            S::Future: Send + 'static,
        {
            on_service(MethodFilter::$method, service)
        }
    };
}

top_level_handler_fn!(get, GET);
top_level_handler_fn!(post, POST);
top_level_handler_fn!(put, PUT);
top_level_handler_fn!(patch, PATCH);
top_level_handler_fn!(delete, DELETE);
top_level_handler_fn!(head, HEAD);
top_level_handler_fn!(options, OPTIONS);
top_level_handler_fn!(trace, TRACE);
top_level_handler_fn!(connect, CONNECT);

top_level_service_fn!(get_service, GET);
top_level_service_fn!(post_service, POST);
top_level_service_fn!(put_service, PUT);
top_level_service_fn!(patch_service, PATCH);
top_level_service_fn!(delete_service, DELETE);
top_level_service_fn!(head_service, HEAD);
top_level_service_fn!(options_service, OPTIONS);
top_level_service_fn!(trace_service, TRACE);
top_level_service_fn!(connect_service, CONNECT);

#[track_caller]
pub fn any<H, T>(handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().any(handler)
}

#[track_caller]
pub fn any_service<S>(service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().any_service(service)
}

#[track_caller]
pub fn on<H, T>(filter: MethodFilter, handler: H) -> MethodRouter
where
    H: Handler<T>,
{
    MethodRouter::new().on(filter, handler)
}

#[track_caller]
pub fn on_service<S>(filter: MethodFilter, service: S) -> MethodRouter
where
    S: Service<IncomingRequest, Response = HttpResponse, Error = Infallible>
        + Clone
        + Send
        + Sync
        + 'static,
    S::Future: Send + 'static,
{
    MethodRouter::new().on_service(filter, service)
}
