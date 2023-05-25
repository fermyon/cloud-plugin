use anyhow::{anyhow, Context, Result};
use clap::Parser;
use cloud::client::{Client as CloudClient, ConnectionConfig};
use spin_common::arg_parser::parse_kv;
use uuid::Uuid;

use crate::{
    commands::deploy::{get_app_id_cloud, login_connection},
    opts::*,
};

/// Manage Spin application variables
#[derive(Parser, Debug)]
#[clap(about = "Log into the Fermyon Platform")]
pub struct VariablesCommand {
    /// Variable pair to set
    #[clap(name = VARIABLES_SET_OPT, short = 's', long = "set", parse(try_from_str = parse_kv))]
    pub variables_to_set: Vec<(String, String)>,

    /// Deploy to the Fermyon instance saved under the specified name.
    /// If omitted, Spin deploys to the default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV
    )]
    pub deployment_env_id: Option<String>,

    /// Name of Spin app
    #[clap(name = "app", long = "app")]
    pub app: String,
}

impl VariablesCommand {
    pub async fn run(self) -> Result<()> {
        let login_connection = login_connection(self.deployment_env_id.as_deref()).await?;

        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };

        let client = CloudClient::new(connection_config.clone());

        let app_id = get_app_id_cloud(&client, self.app.clone())
            .await
            .with_context(|| anyhow!(format!("Could not find app_id for app {}", self.app)))?;

        set_variables(&client, app_id, &self.variables_to_set).await?;
        Ok(())
    }
}

pub(crate) async fn set_variables(
    client: &CloudClient,
    app_id: Uuid,
    variables: &[(String, String)],
) -> Result<()> {
    for var in variables {
        CloudClient::add_variable_pair(client, app_id, var.0.to_owned(), var.1.to_owned())
            .await
            .context("Problem creating variable")?;
    }
    Ok(())
}
