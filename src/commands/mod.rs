pub mod deploy;
pub mod login;
pub mod sqlite;
pub mod variables;

use crate::commands::deploy::login_connection;
use anyhow::Result;
use cloud::client::{Client as CloudClient, ConnectionConfig};

pub(crate) async fn create_cloud_client(deployment_env_id: Option<&str>) -> Result<CloudClient> {
    let login_connection = login_connection(deployment_env_id).await?;
    let connection_config = ConnectionConfig {
        url: login_connection.url.to_string(),
        insecure: login_connection.danger_accept_invalid_certs,
        token: login_connection.token,
    };
    Ok(CloudClient::new(connection_config))
}
