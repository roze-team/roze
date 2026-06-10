#![allow(dead_code, unused_imports)]

pub mod user;
pub use user::{
    ActiveModel as UserActiveModel, Entity as UserEntity, Model as UserModel, UserRepository,
};
