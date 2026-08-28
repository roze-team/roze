#![allow(dead_code)]

use roze_validation::Validate;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, Validate)]
pub struct LoginResp {
    pub token: String,
    pub expires_at: u64,
}
