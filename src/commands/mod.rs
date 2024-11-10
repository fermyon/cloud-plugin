pub mod apps;
pub mod apps_output;
pub mod deploy;
pub mod key_value;
pub mod link;
pub mod links_output;
pub mod links_target;
pub mod login;
pub mod logs;
pub mod sqlite;
pub mod variables;

use crate::{commands::deploy::login_connection, opts::DEPLOYMENT_ENV_NAME_ENV};
use anyhow::{Context, Result};
use clap::Args;
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

#[derive(Debug, Default, Args)]
struct CommonArgs {
    /// Deploy to the Fermyon instance saved under the specified name.
    /// If omitted, Spin deploys to the default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV,
        hidden = true
    )]
    pub deployment_env_id: Option<String>,
}

fn disallow_empty(statement: &str) -> anyhow::Result<String> {
    if statement.trim().is_empty() {
        anyhow::bail!("cannot be empty");
    }
    return Ok(statement.trim().to_owned());
}
