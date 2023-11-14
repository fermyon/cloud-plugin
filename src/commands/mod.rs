pub mod apps;
pub mod deploy;
pub mod link;
pub mod login;
pub mod logs;
pub mod sqlite;
pub mod variables;

use crate::commands::deploy::login_connection;
use anyhow::{Context, Result};
use cloud::{
    client::{Client as CloudClient, ConnectionConfig},
    CloudClientExt,
};
use uuid::Uuid;

const DEFAULT_CLOUD_URL: &str = "https://cloud.fermyon.com/";

pub(crate) async fn create_cloud_client(deployment_env_id: Option<&str>) -> Result<CloudClient> {
    let login_connection = login_connection(deployment_env_id).await?;
    let connection_config = ConnectionConfig {
        url: login_connection.url.to_string(),
        insecure: login_connection.danger_accept_invalid_certs,
        token: login_connection.token,
    };
    Ok(CloudClient::new(connection_config))
}

async fn client_and_app_id(
    deployment_env_id: Option<&str>,
    app: &str,
) -> Result<(CloudClient, Uuid)> {
    let client = create_cloud_client(deployment_env_id).await?;
    let app_id = client
        .get_app_id(app)
        .await
        .with_context(|| format!("Error finding app_id for app '{}'", app))?
        .with_context(|| format!("Could not find app '{}'", app))?;
    Ok((client, app_id))
}
