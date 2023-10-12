use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use uuid::Uuid;

use crate::CloudClientInterface;

#[async_trait]
pub trait CloudClientExt {
    async fn add_revision_ref(&self, storage_id: &str, version: &str) -> anyhow::Result<()>;

    async fn get_app_id(&self, app_name: &str) -> Result<Option<Uuid>>;
    async fn get_revision_id(&self, app_id: Uuid, version: &str) -> Result<Uuid>;
    async fn get_channel_id(&self, app_id: Uuid, channel_name: &str) -> Result<Uuid>;

    async fn set_key_values(
        &self,
        app_id: Uuid,
        store_label: &str,
        key_values: &[(String, String)],
    ) -> Result<()>;
    async fn set_variables(&self, app_id: Uuid, variables: &[(String, String)]) -> Result<()>;
}

#[async_trait]
impl<T: CloudClientInterface> CloudClientExt for T {
    async fn add_revision_ref(&self, storage_id: &str, version: &str) -> anyhow::Result<()> {
        self.add_revision(storage_id.to_owned(), version.to_owned())
            .await
    }

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

    async fn get_channel_id(&self, app_id: Uuid, channel_name: &str) -> Result<Uuid> {
        let mut channels_vm = self.list_channels().await?;

        loop {
            if let Some(channel) = channels_vm
                .items
                .iter()
                .find(|&x| x.app_id == app_id && x.name == channel_name)
            {
                return Ok(channel.id);
            }

            if channels_vm.is_last_page {
                break;
            }

            channels_vm = self.list_channels_next(&channels_vm).await?;
        }

        Err(anyhow!(
            "No channel with app_id {} and name {}",
            app_id,
            channel_name,
        ))
    }

    async fn set_key_values(
        &self,
        app_id: Uuid,
        store_label: &str,
        key_values: &[(String, String)],
    ) -> Result<()> {
        for (key, value) in key_values {
            self.add_key_value_pair(
                app_id,
                store_label.to_owned(),
                key.to_owned(),
                value.to_owned(),
            )
            .await
            .with_context(|| format!("Problem creating key/value {}", key))?;
        }
        Ok(())
    }

    async fn set_variables(&self, app_id: Uuid, variables: &[(String, String)]) -> Result<()> {
        for (name, value) in variables {
            self.add_variable_pair(app_id, name.to_owned(), value.to_owned())
                .await
                .with_context(|| format!("Problem creating variable {}", name))?;
        }
        Ok(())
    }
}
