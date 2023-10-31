use anyhow::Result;
use async_trait::async_trait;
use cloud_openapi::models::{
    AppItem, AppItemPage, ChannelItem, ChannelItemPage, ChannelRevisionSelectionStrategy, Database,
    DeviceCodeItem, EnvironmentVariableItem, GetAppLogsVm, GetAppRawLogsVm, ResourceLabel,
    RevisionItemPage, TokenInfo,
};

use std::string::String;
use uuid::Uuid;

#[cfg_attr(feature = "mocks", mockall::automock)]
#[async_trait]
pub trait CloudClientInterface: Send + Sync {
    async fn create_device_code(&self, client_id: Uuid) -> Result<DeviceCodeItem>;

    async fn login(&self, token: String) -> Result<TokenInfo>;

    async fn refresh_token(&self, token: String, refresh_token: String) -> Result<TokenInfo>;

    async fn add_app(&self, name: &str, storage_id: &str) -> Result<Uuid>;

    async fn remove_app(&self, id: String) -> Result<()>;

    async fn get_app(&self, id: String) -> Result<AppItem>;

    async fn list_apps(&self, page_size: i32, page_index: Option<i32>) -> Result<AppItemPage>;

    async fn app_logs(&self, id: String) -> Result<GetAppLogsVm>;

    async fn app_logs_raw(
        &self,
        id: String,
        max_lines: Option<i32>,
        since: Option<String>,
    ) -> Result<GetAppRawLogsVm>;

    async fn get_channel_by_id(&self, id: &str) -> Result<ChannelItem>;

    async fn list_channels(&self) -> Result<ChannelItemPage>;

    async fn list_channels_next(&self, previous: &ChannelItemPage) -> Result<ChannelItemPage>;

    async fn add_channel(
        &self,
        app_id: Uuid,
        name: String,
        revision_selection_strategy: ChannelRevisionSelectionStrategy,
        range_rule: Option<String>,
        active_revision_id: Option<Uuid>,
    ) -> anyhow::Result<Uuid>;

    async fn patch_channel(
        &self,
        id: Uuid,
        name: Option<String>,
        revision_selection_strategy: Option<ChannelRevisionSelectionStrategy>,
        range_rule: Option<String>,
        active_revision_id: Option<Uuid>,
        environment_variables: Option<Vec<EnvironmentVariableItem>>,
    ) -> anyhow::Result<()>;

    async fn remove_channel(&self, id: String) -> Result<()>;

    async fn add_revision(
        &self,
        app_storage_id: String,
        revision_number: String,
    ) -> anyhow::Result<()>;

    async fn list_revisions(&self) -> anyhow::Result<RevisionItemPage>;

    async fn list_revisions_next(
        &self,
        previous: &RevisionItemPage,
    ) -> anyhow::Result<RevisionItemPage>;

    async fn add_key_value_pair(
        &self,
        app_id: Uuid,
        store_name: String,
        key: String,
        value: String,
    ) -> anyhow::Result<()>;

    async fn add_variable_pair(
        &self,
        app_id: Uuid,
        variable: String,
        value: String,
    ) -> anyhow::Result<()>;

    async fn delete_variable_pair(&self, app_id: Uuid, variable: String) -> anyhow::Result<()>;

    async fn get_variable_pairs(&self, app_id: Uuid) -> anyhow::Result<Vec<String>>;

    async fn create_database(
        &self,
        name: String,
        resource_label: Option<ResourceLabel>,
    ) -> anyhow::Result<()>;

    async fn execute_sql(&self, database: String, statement: String) -> anyhow::Result<()>;

    async fn delete_database(&self, name: String) -> anyhow::Result<()>;

    async fn get_databases(&self, app_id: Option<Uuid>) -> anyhow::Result<Vec<Database>>;

    async fn create_database_link(
        &self,
        database: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()>;

    async fn remove_database_link(
        &self,
        database: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()>;

    async fn rename_database(&self, database: String, new_name: String) -> anyhow::Result<()>;
}
