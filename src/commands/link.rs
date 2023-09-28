use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
use cloud_openapi::models::{Database, ResourceLabel};

use crate::commands::{client_and_app_id, sqlite::find_database_link};
use crate::opts::*;

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
        let (client, app_id) =
            client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        let database = CloudClient::get_databases(&client, None)
            .await
            .context("could not fetch databases")?
            .into_iter()
            .find(|d| d.name == self.database);
        if database.is_none() {
            anyhow::bail!(r#"Database "{}" does not exist"#, self.database)
        }

        let databases_for_app = CloudClient::get_databases(&client, Some(app_id))
            .await
            .context("Problem listing links")?;
        let (this_db, other_dbs): (Vec<&Database>, Vec<&Database>) = databases_for_app
            .iter()
            .partition(|d| d.name == self.database);
        let existing_link_for_database = this_db
            .iter()
            .find_map(|d| find_database_link(d, &self.label));
        let existing_link_for_other_database = other_dbs
            .iter()
            .find_map(|d| find_database_link(d, &self.label));
        match (existing_link_for_database, existing_link_for_other_database) {
            (Some(link), _) => {
                anyhow::bail!(
                    r#"A Database is already linked to app "{}" with the label "{}""#,
                    link.app_name(),
                    link.resource_label.label,
                );
            }
            (_, Some(link)) => {
                anyhow::bail!(
                    r#"Database "{}" is already linked to app "{}" with label "{}""#,
                    link.resource,
                    link.app_name(),
                    link.resource_label.label,
                );
            }
            (None, None) => {
                let resource_label = ResourceLabel {
                    app_id,
                    label: self.label,
                    app_name: Some(Some(self.app)),
                };
                CloudClient::create_database_link(&client, &self.database, resource_label).await?;
            }
        }
        Ok(())
    }
}

/// A Link structure to ease grouping a resource with it's app and label
#[derive(Clone, PartialEq)]
pub struct Link {
    pub resource_label: ResourceLabel,
    pub resource: String,
}

impl Link {
    pub fn new(resource_label: ResourceLabel, resource: String) -> Self {
        Self {
            resource_label,
            resource,
        }
    }

    pub fn app_name(&self) -> &str {
        match self.resource_label.app_name.as_ref() {
            Some(Some(a)) => a.as_str(),
            _ => "UNKNOWN",
        }
    }
}
