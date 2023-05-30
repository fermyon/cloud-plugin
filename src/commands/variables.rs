use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser};
use cloud::client::{Client as CloudClient, ConnectionConfig};
use spin_common::arg_parser::parse_kv;
use uuid::Uuid;

use crate::{
    commands::deploy::{get_app_id_cloud, login_connection},
    opts::*,
};

/// Manage Spin application variables
#[derive(Parser, Debug)]
#[clap(about = "Manage Spin application variables")]
pub enum VariablesCommand {
    /// Set variable pairs
    Set(SetCommand),
}

#[derive(Parser, Debug)]
pub struct SetCommand {
    /// Variable pair to set
    #[clap(name = VARIABLES_SET_OPT, parse(try_from_str = parse_kv))]
    pub variables_to_set: Vec<(String, String)>,
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Default, Args)]
struct CommonArgs {
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

/// Information needed to connect to Fermyon Cloud to manage a specific Spin application
struct AppManagmentInfo {
    app_id: Uuid,
    client: CloudClient,
}

impl AppManagmentInfo {
    pub async fn new(deployment_env_id: Option<&str>, app: &str) -> Result<Self> {
        let login_connection = login_connection(deployment_env_id).await?;
        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };
        let client = CloudClient::new(connection_config.clone());
        let app_id = get_app_id_cloud(&client, app.to_string())
            .await
            .with_context(|| anyhow!(format!("Could not find app_id for app {}", app)))?;
        Ok(AppManagmentInfo { app_id, client })
    }
}

impl VariablesCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Set(cmd) => {
                let info =
                    AppManagmentInfo::new(cmd.common.deployment_env_id.as_deref(), &cmd.common.app)
                        .await?;
                set_variables(&info.client, info.app_id, &cmd.variables_to_set).await?;
            }
        }
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
