pub mod body;
pub mod error;
pub mod extract;
pub mod handler;
pub mod response;
pub mod rest;
pub mod router;

pub use error::*;
pub use extract::*;
pub use handler::*;
pub use response::*;
pub use rest::*;
pub use router::*;

pub mod http {
    pub use ::http::*;
}

pub mod error_handling {
    pub use crate::error::{handle_error, HandleErrorLayer, HandleErrorService};
}

pub mod routing {
    pub use crate::router::{
        any, any_service, connect, connect_service, delete, delete_service, get, get_service, head,
        head_service, on, on_service, options, options_service, patch, patch_service, post,
        post_service, put, put_service, trace, trace_service, MethodFilter, MethodRouter,
    };
}
