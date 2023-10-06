use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::CloudClientInterface;
use cloud_openapi::models::{Database, ResourceLabel};
use uuid::Uuid;

use crate::commands::{client_and_app_id, sqlite::find_database_link};
use crate::opts::*;

/// Manage how apps and resources are linked together
#[derive(Parser, Debug)]
pub enum LinkCommand {
    /// Link an app to a NoOps SQL database
    Sqlite(SqliteLinkCommand),
}

#[derive(Parser, Debug)]
pub struct SqliteLinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// The name by which the application will refer to the database
    label: String,
    #[clap(short = 'a', long = "app")]
    /// The app that will be using the database
    app: String,
    /// The database that the app will refer to by the label
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
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                cmd.link(client, app_id).await
            }
        }
    }
}

impl SqliteLinkCommand {
    async fn link(self, client: impl CloudClientInterface, app_id: Uuid) -> Result<()> {
        let database = client
            .get_databases(None)
            .await
            .context("could not fetch databases")?
            .into_iter()
            .find(|d| d.name == self.database);
        if database.is_none() {
            anyhow::bail!(r#"Database "{}" does not exist"#, self.database)
        }

        let databases_for_app = client
            .get_databases(Some(app_id))
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
        let success_msg = format!(
            r#"Database "{}" is now linked to app "{}" with the label "{}""#,
            self.database, self.app, self.label
        );
        match (existing_link_for_database, existing_link_for_other_database) {
            (Some(link), _) => {
                anyhow::bail!(
                    r#"Database "{}" is already linked to app "{}" with the label "{}""#,
                    link.resource,
                    link.app_name(),
                    link.resource_label.label,
                );
            }
            (_, Some(link)) => {
                let prompt = format!(
                    r#"App "{}"'s "{}" label is currently linked to "{}". Change to link to database "{}" instead?"#,
                    link.app_name(),
                    link.resource_label.label,
                    link.resource,
                    self.database,
                );
                if dialoguer::Confirm::new()
                    .with_prompt(prompt)
                    .default(false)
                    .interact_opt()?
                    .unwrap_or_default()
                {
                    // TODO: use a relink API to remove any downtime
                    client
                        .remove_database_link(&link.resource, link.resource_label)
                        .await?;
                    let resource_label = ResourceLabel {
                        app_id,
                        label: self.label,
                        app_name: None,
                    };
                    client
                        .create_database_link(&self.database, resource_label)
                        .await?;
                    println!("{success_msg}");
                } else {
                    println!("The link has not been updated");
                }
            }
            (None, None) => {
                let resource_label = ResourceLabel {
                    app_id,
                    label: self.label,
                    app_name: None,
                };
                client
                    .create_database_link(&self.database, resource_label)
                    .await?;
                println!("{success_msg}");
            }
        }
        Ok(())
    }
}

/// Manage unlinking apps and resources
#[derive(Parser, Debug)]
pub enum UnlinkCommand {
    /// Unlink an app from a NoOps SQL database
    Sqlite(SqliteUnlinkCommand),
}

impl UnlinkCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Sqlite(cmd) => cmd.unlink().await,
        }
    }
}

#[derive(Parser, Debug)]
pub struct SqliteUnlinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// The name by which the application refers to the database
    label: String,
    #[clap(short = 'a', long = "app")]
    /// The app that will be using the database
    app: String,
}

impl SqliteUnlinkCommand {
    async fn unlink(self) -> Result<()> {
        let (client, app_id) =
            client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        let (database, label) = client
            .get_databases(Some(app_id))
            .await
            .context("could not fetch databases")?
            .into_iter()
            .find_map(|d| {
                d.links
                    .into_iter()
                    .find(|l| {
                        matches!(&l.app_name, Some(app_name) if app_name == &self.app)
                            && l.label == self.label
                    })
                    .map(|l| (d.name, l))
            })
            .with_context(|| {
                format!(
                    "no database was linked to app '{}' with label '{}'",
                    self.app, self.label
                )
            })?;

        client.remove_database_link(&database, label).await?;
        println!("Database '{database}' no longer linked to app {}", self.app);
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
            Some(a) => a.as_str(),
            _ => "UNKNOWN",
        }
    }
}

#[cfg(test)]
mod link_tests {
    use super::*;
    use cloud::MockCloudClientInterface;
    #[tokio::test]
    async fn test_sqlite_link_error_database_does_not_exist() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = SqliteLinkCommand {
            app: "app".to_string(),
            database: "does-not-exist".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            Database::new("db1".to_string(), vec![]),
            Database::new("db2".to_string(), vec![]),
        ];
        mock.expect_get_databases()
            .returning(move |_| Ok(dbs.to_owned()));

        let result = command.link(mock, app_id).await;
        match result {
            Ok(_) => panic!("Expected error"),
            Err(err) => assert_eq!(
                err.to_string(),
                r#"Database "does-not-exist" does not exist"#
            ),
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_link_success() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = SqliteLinkCommand {
            app: "app".to_string(),
            database: "db1".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            Database::new("db1".to_string(), vec![]),
            Database::new("db2".to_string(), vec![]),
        ];
        mock.expect_get_databases()
            .returning(move |_| Ok(dbs.to_owned()));
        let expected_resource_label = ResourceLabel {
            app_id,
            label: command.label.clone(),
            app_name: None,
        };
        mock.expect_create_database_link()
            .withf(move |db, rl| db == "db1" && rl == &expected_resource_label)
            .returning(|_, _| Ok(()));
        command.link(mock, app_id).await
    }

    #[tokio::test]
    async fn test_sqlite_link_error_link_exists() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = SqliteLinkCommand {
            app: "app".to_string(),
            database: "db1".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            Database::new(
                "db1".to_string(),
                vec![ResourceLabel {
                    app_id,
                    label: command.label.clone(),
                    app_name: Some("app".to_string()),
                }],
            ),
            Database::new("db2".to_string(), vec![]),
        ];
        mock.expect_get_databases()
            .returning(move |_| Ok(dbs.to_owned()));
        let result = command.link(mock, app_id).await;
        match result {
            Ok(_) => panic!("Expected error"),
            Err(err) => assert_eq!(
                err.to_string(),
                r#"Database "db1" is already linked to app "app" with the label "label""#
            ),
        }
        Ok(())
    }

    // TODO: add test for test_sqlite_link_error_link_exists_different_database() once
    // there is a flag to avoid prompts
}
