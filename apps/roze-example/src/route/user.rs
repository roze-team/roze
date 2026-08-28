use roze_http::{
    routing::{delete, get, head, patch, post, put},
    Router,
};

use crate::handler;

pub fn routes() -> Router {
    Router::new()
        .route("/user/login", post(handler::user::login))
        .route("/user/{id}", get(handler::user::get_user))
}
