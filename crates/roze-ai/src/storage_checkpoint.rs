use std::{fmt::Write as _, sync::Arc};

use async_trait::async_trait;
use roze_context::Context;
use roze_storage::{normalize_object_key, ObjectStorage, PutObjectRequest};
use sha2::{Digest, Sha256};

use crate::{tool::check_context, AiError, CheckpointStore, WorkflowCheckpoint};

/// Durable workflow checkpoints backed by Roze's existing object-storage layer.
///
/// The adapter isolates tenant and subject scopes in hashed object-key prefixes.
/// The configured storage must allow `.json` and `application/json`.
#[derive(Debug, Clone)]
pub struct ObjectStorageCheckpointStore {
    storage: Arc<dyn ObjectStorage>,
    prefix: String,
}

impl ObjectStorageCheckpointStore {
    pub fn new(
        storage: Arc<dyn ObjectStorage>,
        prefix: impl Into<String>,
    ) -> Result<Self, AiError> {
        let prefix = normalize_object_key(prefix.into())
            .map_err(|error| AiError::Checkpoint(format!("invalid storage prefix: {error}")))?;
        Ok(Self { storage, prefix })
    }

    fn key(&self, context: &Context, run_id: &str) -> Result<String, AiError> {
        if run_id.is_empty()
            || !run_id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(AiError::Checkpoint(
                "checkpoint run id contains unsupported characters".to_string(),
            ));
        }
        let tenant = context.tenant().unwrap_or_default();
        let subject = context.subject().unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(tenant.len().to_be_bytes());
        hasher.update(tenant.as_bytes());
        hasher.update(subject.len().to_be_bytes());
        hasher.update(subject.as_bytes());
        let mut scope = String::with_capacity(64);
        for byte in hasher.finalize() {
            write!(&mut scope, "{byte:02x}")
                .map_err(|error| AiError::Internal(format!("failed to encode scope: {error}")))?;
        }
        normalize_object_key(format!("{}/{scope}/{run_id}.json", self.prefix))
            .map_err(|error| AiError::Checkpoint(format!("invalid checkpoint key: {error}")))
    }
}

#[async_trait]
impl CheckpointStore for ObjectStorageCheckpointStore {
    async fn load(
        &self,
        context: &Context,
        run_id: &str,
    ) -> Result<Option<WorkflowCheckpoint>, AiError> {
        check_context(context)?;
        let key = self.key(context, run_id)?;
        if self
            .storage
            .stat_object(&key)
            .await
            .map_err(|error| AiError::Checkpoint(format!("failed to stat checkpoint: {error}")))?
            .is_none()
        {
            return Ok(None);
        }
        let bytes =
            self.storage.get_object(&key).await.map_err(|error| {
                AiError::Checkpoint(format!("failed to load checkpoint: {error}"))
            })?;
        check_context(context)?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| AiError::Checkpoint(format!("invalid checkpoint payload: {error}")))
    }

    async fn save(&self, context: &Context, checkpoint: WorkflowCheckpoint) -> Result<(), AiError> {
        check_context(context)?;
        let key = self.key(context, checkpoint.run_id())?;
        let bytes = serde_json::to_vec(&checkpoint).map_err(|error| {
            AiError::Checkpoint(format!("failed to encode checkpoint: {error}"))
        })?;
        self.storage
            .put_object(PutObjectRequest {
                key,
                bytes,
                content_type: Some("application/json".to_string()),
                metadata: Default::default(),
            })
            .await
            .map_err(|error| AiError::Checkpoint(format!("failed to save checkpoint: {error}")))?;
        check_context(context)
    }

    async fn delete(&self, context: &Context, run_id: &str) -> Result<(), AiError> {
        check_context(context)?;
        let key = self.key(context, run_id)?;
        self.storage.delete_object(&key).await.map_err(|error| {
            AiError::Checkpoint(format!("failed to delete checkpoint: {error}"))
        })?;
        check_context(context)
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, time::SystemTime};

    use roze_storage::{LocalObjectStorage, StorageConfig, StorageProvider, StorageValidation};
    use serde_json::json;

    use super::*;

    fn local_storage() -> (Arc<LocalObjectStorage>, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "roze-ai-checkpoints-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let storage = Arc::new(LocalObjectStorage::new(StorageConfig {
            provider: StorageProvider::Local,
            bucket: "test".to_string(),
            root: root.clone(),
            validation: StorageValidation {
                max_size_bytes: 1024 * 1024,
                allowed_mime_types: vec!["application/json".to_string()],
                allowed_extensions: vec!["json".to_string()],
            },
            ..StorageConfig::default()
        }));
        (storage, root)
    }

    #[tokio::test]
    async fn object_storage_round_trips_and_deletes_checkpoint() {
        let (storage, root) = local_storage();
        let store =
            ObjectStorageCheckpointStore::new(storage, "ai/checkpoints").expect("checkpoint store");
        let context = Context::background();
        let checkpoint = WorkflowCheckpoint {
            version: 1,
            run_id: "run-1".to_string(),
            graph_revision: "v1".to_string(),
            tenant: context.tenant(),
            subject: context.subject(),
            next_node_index: 1,
            values: [("prepare".to_string(), json!("done"))].into(),
            interrupted_before: None,
        };

        store
            .save(&context, checkpoint.clone())
            .await
            .expect("save");
        assert_eq!(
            store.load(&context, "run-1").await.expect("load"),
            Some(checkpoint)
        );
        store.delete(&context, "run-1").await.expect("delete");
        assert_eq!(store.load(&context, "run-1").await.expect("load"), None);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn object_storage_rejects_unsafe_run_ids() {
        let (storage, root) = local_storage();
        let store =
            ObjectStorageCheckpointStore::new(storage, "ai/checkpoints").expect("checkpoint store");
        assert!(store.key(&Context::background(), "../other").is_err());
        std::fs::remove_dir_all(root).ok();
    }
}
