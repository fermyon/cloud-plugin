use crate::commands::create_cloud_client;
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;

/// Manage links between apps and resources
#[derive(Parser, Debug)]
#[clap(about = "Manage links between apps and resources")]
pub enum LinkCommand {
    Sqlite(SqliteLinkCommand),
}

#[derive(Parser, Debug)]
pub struct SqliteLinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    link: String,
    #[clap(short = 'a', long = "app")]
    app: String,
    #[clap(short = 'd', long = "database")]
    database: String,
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

impl LinkCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Sqlite(cmd) => {
                // let (client, app_id) = client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let app_id = uuid::Uuid::new_v4();

                let dbs = CloudClient::get_databases(&client, Some(app_id))
                    .await
                    .context("Problem listing databases")?;

                if dbs
                    .into_iter()
                    .any(|db| db.links.iter().any(|l| l.name == cmd.link))
                {
                    anyhow::bail!(
                        "No link found with name \"{}\" for app \"{}\"",
                        cmd.link,
                        cmd.app
                    );
                } else {
                    CloudClient::create_link(
                        &client,
                        &cmd.link,
                        &app_id.to_string(),
                        &cmd.database,
                    )
                    .await?;
                }
                Ok(())
            }
        }
    }
}
