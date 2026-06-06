#![allow(dead_code, unused_imports)]

use std::time::Duration;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DeleteResult, EntityTrait, IntoActiveModel};
use serde::{Deserialize, Serialize};

use crate::svc::ServiceContext;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, DeriveEntityModel)]
#[sea_orm(table_name = "users")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub username: String,
    pub password: String,
    pub created_at: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

pub struct UserRepository<'a> {
    ctx: &'a ServiceContext,
}

impl<'a> UserRepository<'a> {
    pub fn new(ctx: &'a ServiceContext) -> Self {
        Self { ctx }
    }

    fn db(&self) -> anyhow::Result<&DatabaseConnection> {
        self.ctx.db.as_ref().ok_or_else(|| anyhow::anyhow!("database connection is not configured"))
    }

    pub fn table_name() -> &'static str {
        "users"
    }

    pub async fn find_by_id(&self, id: i64) -> anyhow::Result<Option<Model>> {
        let db = self.db()?;
        Ok(Entity::find_by_id(id).one(db).await?)
    }

    pub async fn list(&self) -> anyhow::Result<Vec<Model>> {
        let db = self.db()?;
        Ok(Entity::find().all(db).await?)
    }

    pub async fn insert(&self, model: Model) -> anyhow::Result<Model> {
        let db = self.db()?;
        let active: ActiveModel = model.into_active_model();
        let inserted = active.insert(db).await?;
        self.invalidate_cache(inserted.id).await?;
        Ok(inserted)
    }

    pub async fn update(&self, model: Model) -> anyhow::Result<Model> {
        let db = self.db()?;
        let active: ActiveModel = model.into_active_model();
        let updated = active.update(db).await?;
        self.invalidate_cache(updated.id).await?;
        Ok(updated)
    }

    pub async fn delete_by_id(&self, id: i64) -> anyhow::Result<DeleteResult> {
        let db = self.db()?;
        let result = Entity::delete_by_id(id).exec(db).await?;
        self.invalidate_cache(id).await?;
        Ok(result)
    }

    fn cache_key(&self, id: i64) -> String {
        format!("{}:{}", Self::table_name(), id)
    }

    async fn invalidate_cache(&self, id: i64) -> anyhow::Result<()> {
        if let Some(cache) = self.ctx.cache.as_ref() {
            let key = self.cache_key(id);
            cache.del(&key).await?;
        }
        Ok(())
    }

    pub async fn cached_find_by_id(&self, id: i64) -> anyhow::Result<Option<Model>> {
        if let Some(cache) = self.ctx.cache.as_ref() {
            let key = self.cache_key(id);
            let ttl = Duration::from_secs(300);
            let negative_ttl = Duration::from_secs((300 / 6).clamp(5, 60));
            return cache
                .get_or_set_json_option(
                    &key,
                    Some(ttl),
                    Some(negative_ttl),
                    || async { self.find_by_id(id).await },
                )
                .await;
        }

        self.find_by_id(id).await
    }
}
