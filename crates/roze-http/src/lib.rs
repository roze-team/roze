pub mod extract;
pub mod handler;
pub mod response;
pub mod rest;
pub mod router;

pub use extract::*;
pub use handler::*;
pub use response::*;
pub use rest::*;
pub use router::*;

pub mod http {
    pub use ::http::*;
}

pub mod routing {
    pub use crate::router::{
        any, any_service, connect, delete, get, head, options, patch, post, put, trace,
        MethodRouter,
    };
}
