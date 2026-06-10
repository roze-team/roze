#![allow(dead_code)]

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResp {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetUserReq {
    pub id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResp {
    pub id: i64,
    pub username: String,
    pub created_at: i64,
}
