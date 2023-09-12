use crate::commands::create_cloud_client;
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::{client::Client as CloudClient, mocks::Database};

/// Manage how apps and resources are linked together
#[derive(Parser, Debug)]
pub enum LinkCommand {
    Sqlite(SqliteLinkCommand),
}

#[derive(Parser, Debug)]
pub struct SqliteLinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    // TODO: validate link syntax
    link: String,
    #[clap(short = 'a', long = "app")]
    app: String,
    #[clap(short = 'd', long = "database")]
    database: String,
    #[clap(short = 'r', long = "remove", takes_value = false)]
    remove: bool,
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
            Self::Sqlite(cmd) => cmd.link().await,
        }
    }
}

impl SqliteLinkCommand {
    async fn link(self) -> Result<()> {
        // let (client, app_id) = client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let app_id = uuid::Uuid::new_v4();

        let dbs = CloudClient::get_databases(&client, Some(app_id))
            .await
            .context("Problem listing databases")?;
        let existing_linked_db: Option<Database> = dbs
            .into_iter()
            .find(|db| db.links.iter().any(|l| l.label == self.link));
        match existing_linked_db {
            Some(db) => {
                if self.remove {
                    CloudClient::remove_link(
                        &client,
                        &self.link,
                        &app_id.to_string(),
                        &self.database,
                    )
                    .await?;
                } else if db.name == self.database {
                    anyhow::bail!(
                        "Link \"{}\" already exists for app \"{}\" and database \"{}\"",
                        self.link,
                        self.app,
                        self.database,
                    );
                } else {
                    let res = dialoguer::Confirm::new()
                        .with_prompt(format!(
                            "Link \"{}\" already exists for app \"{}\" with database \"{}\"",
                            self.link, self.app, self.database
                        ))
                        .default(true)
                        .interact_opt()?;
                    if let Some(update) = res {
                        if update {
                            CloudClient::create_link(
                                &client,
                                &self.link,
                                &app_id.to_string(),
                                &self.database,
                            )
                            .await?;
                        } else {
                            println!("Link will not be updated.")
                        }
                    }
                }
            }
            None => {
                if self.remove {
                    println!(
                        "Link \"{}\" does not exist for app \"{}\" and database \"{}\"",
                        self.link, self.app, self.database,
                    );
                } else {
                    CloudClient::create_link(
                        &client,
                        &self.link,
                        &app_id.to_string(),
                        &self.database,
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }
}
