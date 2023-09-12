pub mod apps;
pub mod deploy;
pub mod link;
pub mod login;
pub mod sqlite;
pub mod variables;

use crate::{commands::deploy::login_connection, opts::DEFAULT_APPLIST_PAGE_SIZE};
use anyhow::{Context, Result};
use cloud::client::{Client as CloudClient, ConnectionConfig};
use uuid::Uuid;

pub(crate) async fn create_cloud_client(deployment_env_id: Option<&str>) -> Result<CloudClient> {
    let login_connection = login_connection(deployment_env_id).await?;
    let connection_config = ConnectionConfig {
        url: login_connection.url.to_string(),
        insecure: login_connection.danger_accept_invalid_certs,
        token: login_connection.token,
    };
    Ok(CloudClient::new(connection_config))
}

pub(crate) async fn get_app_id_cloud(
    cloud_client: &CloudClient,
    name: &str,
) -> Result<Option<Uuid>> {
    let apps_vm = CloudClient::list_apps(cloud_client, DEFAULT_APPLIST_PAGE_SIZE, None)
        .await
        .context("Could not fetch apps")?;
    let app = apps_vm.items.iter().find(|&x| x.name == name);
    Ok(app.map(|a| a.id))
}

async fn client_and_app_id(
    deployment_env_id: Option<&str>,
    app: &str,
) -> Result<(CloudClient, Uuid)> {
    let client = create_cloud_client(deployment_env_id).await?;
    let app_id = get_app_id_cloud(&client, app)
        .await?
        .with_context(|| format!("Could not find app_id for app {}", app))?;
    Ok((client, app_id))
}
