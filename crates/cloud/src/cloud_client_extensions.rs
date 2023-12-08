use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use uuid::Uuid;

use crate::CloudClientInterface;

#[async_trait]
pub trait CloudClientExt {
    async fn get_app_id(&self, app_name: &str) -> Result<Option<Uuid>>;
    async fn get_revision_id(&self, app_id: Uuid, version: &str) -> Result<Uuid>;
}

#[async_trait]
impl<T: CloudClientInterface> CloudClientExt for T {
    async fn get_app_id(&self, app_name: &str) -> Result<Option<Uuid>> {
        let apps_vm = self
            .list_apps(crate::DEFAULT_APPLIST_PAGE_SIZE, None)
            .await
            .context("Could not fetch apps")?;
        let app = apps_vm.items.iter().find(|&x| x.name == app_name);
        Ok(app.map(|a| a.id))
    }

    async fn get_revision_id(&self, app_id: Uuid, version: &str) -> Result<Uuid> {
        let mut revisions = self.list_revisions().await?;

        loop {
            if let Some(revision) = revisions
                .items
                .iter()
                .find(|&x| x.revision_number == version && x.app_id == app_id)
            {
                return Ok(revision.id);
            }

            if revisions.is_last_page {
                break;
            }

            revisions = self.list_revisions_next(&revisions).await?;
        }

        Err(anyhow!(
            "No revision with version {} and app id {}",
            version,
            app_id
        ))
    }
}
