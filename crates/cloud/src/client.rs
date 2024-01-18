use anyhow::{Context, Result};
use async_trait::async_trait;
use cloud_openapi::{
    apis::{
        apps_api::{
            api_apps_get, api_apps_id_delete, api_apps_id_get, api_apps_id_logs_get,
            api_apps_id_logs_raw_get, api_apps_post,
        },
        auth_tokens_api::api_auth_tokens_refresh_post,
        configuration::{ApiKey, Configuration},
        device_codes_api::api_device_codes_post,
        key_value_pairs_api::api_key_value_pairs_post,
        key_value_stores_api::{
            api_key_value_stores_get, api_key_value_stores_store_delete,
            api_key_value_stores_store_links_delete, api_key_value_stores_store_links_post,
            api_key_value_stores_store_post,
        },
        revisions_api::{api_revisions_get, api_revisions_post},
        sql_databases_api::{
            api_sql_databases_create_post, api_sql_databases_database_links_delete,
            api_sql_databases_database_links_post, api_sql_databases_database_rename_patch,
            api_sql_databases_delete, api_sql_databases_execute_post, api_sql_databases_get,
        },
        variable_pairs_api::{
            api_variable_pairs_delete, api_variable_pairs_get, api_variable_pairs_post,
        },
        Error,
    },
    models::{
        AppItem, AppItemPage, ChannelRevisionSelectionStrategy, CreateAppCommand,
        CreateDeviceCodeCommand, CreateKeyValuePairCommand, CreateSqlDatabaseCommand,
        CreateVariablePairCommand, Database, DeleteSqlDatabaseCommand, DeleteVariablePairCommand,
        DeviceCodeItem, EnvironmentVariableItem, ExecuteSqlStatementCommand, GetAppLogsVm,
        GetAppRawLogsVm, GetSqlDatabasesQuery, GetVariablesQuery, KeyValueStoreItem,
        RefreshTokenCommand, RegisterRevisionCommand, ResourceLabel, RevisionItemPage, TokenInfo,
    },
};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::CloudClientInterface;

const JSON_MIME_TYPE: &str = "application/json";

pub struct Client {
    configuration: Configuration,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct ConnectionConfig {
    pub insecure: bool,
    pub token: String,
    pub url: String,
}

impl Client {
    pub fn new(conn_info: ConnectionConfig) -> Self {
        let mut headers = header::HeaderMap::new();
        headers.insert(header::ACCEPT, JSON_MIME_TYPE.parse().unwrap());
        headers.insert(header::CONTENT_TYPE, JSON_MIME_TYPE.parse().unwrap());

        let base_path = match conn_info.url.strip_suffix('/') {
            Some(s) => s.to_owned(),
            None => conn_info.url,
        };

        let configuration = Configuration {
            base_path,
            user_agent: Some(format!(
                "{}/{} spin/{}",
                env!("CARGO_PKG_NAME"),
                env!("CARGO_PKG_VERSION"),
                std::env::var("SPIN_VERSION").unwrap_or_else(|_| "0".to_string())
            )),
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(conn_info.insecure)
                .default_headers(headers)
                .build()
                .unwrap(),
            basic_auth: None,
            oauth_access_token: None,
            bearer_access_token: None,
            api_key: Some(ApiKey {
                prefix: Some("Bearer".to_owned()),
                key: conn_info.token,
            }),
        };

        Self { configuration }
    }
}

#[async_trait]
impl CloudClientInterface for Client {
    async fn create_device_code(&self, client_id: Uuid) -> Result<DeviceCodeItem> {
        api_device_codes_post(
            &self.configuration,
            CreateDeviceCodeCommand { client_id },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn login(&self, token: String) -> Result<TokenInfo> {
        // When the new OpenAPI specification is released, manually crafting
        // the request should no longer be necessary.
        let response = self
            .configuration
            .client
            .post(format!("{}/api/auth-tokens", self.configuration.base_path))
            .body(
                serde_json::json!(
                    {
                        "provider": "DeviceFlow",
                        "clientId": "583e63e9-461f-4fbe-a246-23e0fb1cad10",
                        "providerCode": token,
                    }
                )
                .to_string(),
            )
            .send()
            .await?;

        serde_json::from_reader(response.bytes().await?.as_ref())
            .context("Failed to parse response")
    }

    async fn refresh_token(&self, token: String, refresh_token: String) -> Result<TokenInfo> {
        api_auth_tokens_refresh_post(
            &self.configuration,
            RefreshTokenCommand {
                token,
                refresh_token,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn add_app(&self, name: &str, storage_id: &str) -> Result<Uuid> {
        api_apps_post(
            &self.configuration,
            CreateAppCommand {
                name: name.to_string(),
                storage_id: storage_id.to_string(),
                create_default_database: None,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn remove_app(&self, id: String) -> Result<()> {
        api_apps_id_delete(&self.configuration, &id, None)
            .await
            .map_err(format_response_error)
    }

    async fn get_app(&self, id: String) -> Result<AppItem> {
        api_apps_id_get(&self.configuration, &id, None)
            .await
            .map_err(format_response_error)
    }

    async fn list_apps(&self, page_size: i32, page_index: Option<i32>) -> Result<AppItemPage> {
        api_apps_get(
            &self.configuration,
            None,
            page_index,
            Some(page_size),
            None,
            None,
            None,
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn app_logs(&self, id: String) -> Result<GetAppLogsVm> {
        api_apps_id_logs_get(&self.configuration, &id, None, None, None)
            .await
            .map_err(format_response_error)
    }

    async fn app_logs_raw(
        &self,
        id: String,
        max_lines: Option<i32>,
        since: Option<String>,
    ) -> Result<GetAppRawLogsVm> {
        api_apps_id_logs_raw_get(&self.configuration, &id, max_lines, since.as_deref(), None)
            .await
            .map_err(format_response_error)
    }

    async fn add_revision(
        &self,
        app_storage_id: String,
        revision_number: String,
    ) -> anyhow::Result<()> {
        api_revisions_post(
            &self.configuration,
            RegisterRevisionCommand {
                app_storage_id,
                revision_number,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn list_revisions(&self) -> anyhow::Result<RevisionItemPage> {
        api_revisions_get(&self.configuration, None, None, None, None)
            .await
            .map_err(format_response_error)
    }

    async fn list_revisions_next(
        &self,
        previous: &RevisionItemPage,
    ) -> anyhow::Result<RevisionItemPage> {
        api_revisions_get(
            &self.configuration,
            Some(previous.page_index + 1),
            Some(previous.page_size),
            None,
            None,
        )
        .await
        .map_err(format_response_error)
    }

    // Key value API methods
    async fn add_key_value_pair(
        &self,
        app_id: Uuid,
        store_name: String,
        key: String,
        value: String,
    ) -> anyhow::Result<()> {
        api_key_value_pairs_post(
            &self.configuration,
            CreateKeyValuePairCommand {
                app_id: Some(app_id),
                store_name: Some(store_name),
                key,
                value,
                label: None,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn create_key_value_store(
        &self,
        store_name: &str,
        resource_label: Option<ResourceLabel>,
    ) -> anyhow::Result<()> {
        api_key_value_stores_store_post(&self.configuration, store_name, None, resource_label)
            .await
            .map_err(format_response_error)
    }

    async fn delete_key_value_store(&self, store_name: &str) -> anyhow::Result<()> {
        api_key_value_stores_store_delete(&self.configuration, store_name, None)
            .await
            .map_err(format_response_error)
    }

    async fn get_key_value_stores(
        &self,
        app_id: Option<Uuid>,
    ) -> anyhow::Result<Vec<KeyValueStoreItem>> {
        let list = api_key_value_stores_get(
            &self.configuration,
            app_id.map(|id| id.to_string()).as_deref(),
            None,
        )
        .await
        .map_err(format_response_error)?;
        Ok(list.key_value_stores)
    }

    async fn create_key_value_store_link(
        &self,
        key_value_store: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()> {
        api_key_value_stores_store_links_post(
            &self.configuration,
            key_value_store,
            resource_label,
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn remove_key_value_store_link(
        &self,
        key_value_store: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()> {
        api_key_value_stores_store_links_delete(
            &self.configuration,
            key_value_store,
            resource_label,
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn add_variable_pair(
        &self,
        app_id: Uuid,
        variable: String,
        value: String,
    ) -> anyhow::Result<()> {
        api_variable_pairs_post(
            &self.configuration,
            CreateVariablePairCommand {
                app_id,
                variable,
                value,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn delete_variable_pair(&self, app_id: Uuid, variable: String) -> anyhow::Result<()> {
        api_variable_pairs_delete(
            &self.configuration,
            DeleteVariablePairCommand { app_id, variable },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn get_variable_pairs(&self, app_id: Uuid) -> anyhow::Result<Vec<String>> {
        let list = api_variable_pairs_get(&self.configuration, GetVariablesQuery { app_id }, None)
            .await
            .map_err(format_response_error)?;
        Ok(list.vars)
    }

    async fn create_database(
        &self,
        name: String,
        resource_label: Option<ResourceLabel>,
    ) -> anyhow::Result<()> {
        let (app_id, label) = match resource_label {
            Some(rl) => (Some(Some(rl.app_id)), Some(Some(rl.label))),
            None => (None, None),
        };
        api_sql_databases_create_post(
            &self.configuration,
            CreateSqlDatabaseCommand {
                name,
                app_id,
                label,
            },
            None,
        )
        .await
        .map_err(format_response_error)
    }

    async fn execute_sql(&self, database: String, statement: String) -> anyhow::Result<()> {
        api_sql_databases_execute_post(
            &self.configuration,
            ExecuteSqlStatementCommand {
                database,
                statement,
                default: false,
            },
            None,
        )
        .await
        .map_err(format_response_error)?;
        Ok(())
    }

    async fn delete_database(&self, name: String) -> anyhow::Result<()> {
        api_sql_databases_delete(&self.configuration, DeleteSqlDatabaseCommand { name }, None)
            .await
            .map_err(format_response_error)
    }

    async fn get_databases(&self, app_id: Option<Uuid>) -> anyhow::Result<Vec<Database>> {
        let list = api_sql_databases_get(
            &self.configuration,
            app_id.map(|id| id.to_string()).as_deref(),
            None,
            // TODO: set to None when the API is updated to not require a body
            Some(GetSqlDatabasesQuery { app_id: None }),
        )
        .await
        .map_err(format_response_error)?;
        Ok(list.databases)
    }

    async fn create_database_link(
        &self,
        database: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()> {
        api_sql_databases_database_links_post(&self.configuration, database, resource_label, None)
            .await
            .map_err(format_response_error)
    }

    async fn remove_database_link(
        &self,
        database: &str,
        resource_label: ResourceLabel,
    ) -> anyhow::Result<()> {
        api_sql_databases_database_links_delete(&self.configuration, database, resource_label, None)
            .await
            .map_err(format_response_error)
    }

    async fn rename_database(&self, database: String, new_name: String) -> anyhow::Result<()> {
        api_sql_databases_database_rename_patch(&self.configuration, &database, &new_name, None)
            .await
            .map_err(format_response_error)
    }
}

#[derive(Deserialize, Debug)]
struct ValidationExceptionMessage {
    title: String,
    errors: HashMap<String, Vec<String>>,
}

#[derive(Deserialize, Debug)]
struct CloudProblemDetails {
    detail: String,
}

fn format_response_error<T>(e: Error<T>) -> anyhow::Error {
    match e {
        Error::ResponseError(r) => {
            // Validation failures are distinguished by the presence of `errors` so try that first
            if let Ok(m) = serde_json::from_str::<ValidationExceptionMessage>(&r.content) {
                anyhow::anyhow!("{} {:?}", m.title, m.errors)
            } else if let Ok(d) = serde_json::from_str::<CloudProblemDetails>(&r.content) {
                anyhow::anyhow!("{}", d.detail)
            } else {
                anyhow::anyhow!("response status code: {}", r.status)
            }
        }
        Error::Serde(err) => {
            anyhow::anyhow!(format!("could not parse JSON object: {}", err))
        }
        _ => anyhow::anyhow!(e.to_string()),
    }
}

#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
struct PatchChannelCommand {
    #[serde(rename = "channelId", skip_serializing_if = "Option::is_none")]
    channel_id: Option<uuid::Uuid>,
    #[serde(
        rename = "environmentVariables",
        skip_serializing_if = "Option::is_none"
    )]
    environment_variables: Option<Vec<EnvironmentVariableItem>>,
    #[serde(rename = "name", skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(
        rename = "revisionSelectionStrategy",
        skip_serializing_if = "Option::is_none"
    )]
    revision_selection_strategy: Option<ChannelRevisionSelectionStrategy>,
    #[serde(rename = "rangeRule", skip_serializing_if = "Option::is_none")]
    range_rule: Option<String>,
    #[serde(rename = "activeRevisionId", skip_serializing_if = "Option::is_none")]
    active_revision_id: Option<uuid::Uuid>,
}
