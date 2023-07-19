use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
use cloud_openapi::models::Database;

use crate::commands::create_cloud_client;
use crate::opts::*;

/// Manage Fermyon Cloud SQL databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQL databases")]
pub enum SqlCommand {
    /// Delete a SQL database
    Delete(DeleteCommand),
    /// List all SQL databases of a user
    List(ListCommand),
}

#[derive(Parser, Debug)]
pub struct DeleteCommand {
    /// Name of database to create
    name: String,
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct ListCommand {
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
}

impl SqlCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Delete(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                CloudClient::delete_database(&client, cmd.name.clone())
                    .await
                    .with_context(|| format!("Problem deleting database {}", cmd.name))?;
            }
            Self::List(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                list_databases(&client).await?;
            }
        }
        Ok(())
    }
}

fn print_databases(databases: Vec<Database>) {
    for d in databases {
        let default_str = if d.default { "(default)" } else { "" };
        println!("{}{default_str}", d.name);
    }
}

pub(crate) async fn list_databases(client: &CloudClient) -> Result<()> {
    let list: Vec<cloud_openapi::models::Database> =
        CloudClient::get_databases(client, None).await?;
    print_databases(list);
    Ok(())
}
