use crate::commands::create_cloud_client;
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::{
    client::Client as CloudClient,
    mocks::{AppLabel, DatabaseLink},
};

/// Manage how apps and resources are linked together
#[derive(Parser, Debug)]
pub enum LinkCommand {
    /// Link an app to a sqlite database
    Sqlite(SqliteLinkCommand),
}

#[derive(Parser, Debug)]
pub struct SqliteLinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// The name by which the application will refer to the database
    // TODO: validate label syntax
    label: String,
    #[clap(short = 'a', long = "app")]
    /// The app that will be using the database
    app: String,
    /// The database that the app will be referring to by the label
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
            Self::Sqlite(cmd) => cmd.link().await,
        }
    }
}

impl SqliteLinkCommand {
    async fn link(self) -> Result<()> {
        // TODO: update go back to this:
        // let (client, app_id) = client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let app_id = uuid::Uuid::new_v4();
        let database = CloudClient::get_database(&client, &self.database)
            .await
            .context("could not fetch database")?;
        if database.is_none() {
            anyhow::bail!(r#"Database "{}" does not exist"#, self.database)
        }

        let links_for_app = CloudClient::list_links(&client, Some(app_id))
            .await
            .context("Problem listing links")?;
        let existing_link_for_database = links_for_app.iter().find(|l| l.database == self.database);
        let existing_link_with_label = links_for_app.iter().find(|l| l.has_label(&self.label));
        match (existing_link_for_database, existing_link_with_label) {
            (Some(link), _) => {
                // TODO: is this so bad? Why not allow linking an app to a database through multiple labels?
                anyhow::bail!(
                    r#"Database "{}" is already linked to app "{}" with label "{}""#,
                    link.database,
                    link.app_label.app_name,
                    link.app_label.label,
                );
            }
            (_, Some(link)) => {
                anyhow::bail!(
                    r#"A Database is already linked to app "{}" with the label "{}""#,
                    link.app_label.app_name,
                    link.app_label.label,
                );
            }
            (None, None) => {
                let link = DatabaseLink::new(
                    AppLabel {
                        app_id,
                        label: self.label,
                        app_name: self.app,
                    },
                    self.database,
                );
                CloudClient::create_link(&client, &link).await?;
            }
        }
        Ok(())
    }
}
