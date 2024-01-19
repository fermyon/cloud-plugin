use crate::commands::links_target::ResourceTarget;
use crate::commands::{create_cloud_client, disallow_empty, CommonArgs};
use anyhow::bail;
use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use cloud::CloudClientInterface;
use cloud_openapi::models::Database;

use std::str::FromStr;

use crate::commands::links_output::{
    print_json, print_table, prompt_delete_resource, ListFormat, ResourceGroupBy, ResourceLinks,
    ResourceType,
};

/// Manage Fermyon Cloud SQLite databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQLite databases")]
pub enum SqliteCommand {
    /// Create a SQLite database
    Create(CreateCommand),
    /// Delete a SQLite database
    Delete(DeleteCommand),
    /// Execute SQL statements against a SQLite database
    Execute(ExecuteCommand),
    /// List all your SQLite databases
    List(ListCommand),
    /// Rename a SQLite database
    Rename(RenameCommand),
}

#[derive(Parser, Debug)]
pub struct CreateCommand {
    /// Name of database to create
    name: String,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct DeleteCommand {
    /// Name of database to delete
    name: String,

    /// Skips prompt to confirm deletion of database
    #[clap(short = 'y', long = "yes", takes_value = false)]
    yes: bool,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct ExecuteCommand {
    /// Name of database to execute against
    #[clap(name = "DATABASE", short = 'd', long = "database", value_parser = clap::builder::ValueParser::new(disallow_empty), group = "db", required_unless_present = "LABEL")]
    database: Option<String>,

    /// Label of database to execute against
    #[clap(name = "LABEL", short = 'l', long = "label", value_parser = clap::builder::ValueParser::new(disallow_empty), group = "db", requires = "APP", required_unless_present = "DATABASE")]
    label: Option<String>,

    /// App to which label relates
    #[clap(name = "APP", short = 'a', long = "app", value_parser = clap::builder::ValueParser::new(disallow_empty), requires = "LABEL", conflicts_with = "DATABASE")]
    app: Option<String>,

    ///Statement to execute
    #[clap(value_parser = clap::builder::ValueParser::new(disallow_empty))]
    statement: String,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct RenameCommand {
    /// Current name of database to rename
    name: String,

    /// New name for the database
    new_name: String,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct ListCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// Filter list by an app
    #[clap(short = 'a', long = "app")]
    app: Option<String>,
    /// Filter list by a database
    #[clap(short = 'd', long = "database")]
    database: Option<String>,
    /// Grouping strategy of tabular list [default: app]
    #[clap(value_enum, short = 'g', long = "group-by")]
    group_by: Option<GroupBy>,
    /// Format of list
    #[clap(value_enum, long = "format", default_value = "table")]
    format: ListFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
enum GroupBy {
    #[default]
    App,
    Database,
}

impl std::fmt::Display for GroupBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GroupBy::App => f.write_str("app"),
            GroupBy::Database => f.write_str("database"),
        }
    }
}

impl FromStr for GroupBy {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "app" => Ok(Self::App),
            "database" => Ok(Self::Database),
            s => Err(format!("Unrecognized group-by option: '{s}'")),
        }
    }
}

impl From<GroupBy> for ResourceGroupBy {
    fn from(group_by: GroupBy) -> Self {
        match group_by {
            GroupBy::App => Self::App,
            GroupBy::Database => Self::Resource(ResourceType::Database),
        }
    }
}

impl SqliteCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Create(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                cmd.run(client).await
            }
            Self::Delete(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                cmd.run(client).await
            }
            Self::Execute(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                cmd.run(client).await
            }
            Self::List(cmd) => cmd.run().await,
            Self::Rename(cmd) => cmd.run().await,
        }
    }
}

impl CreateCommand {
    pub async fn run(self, client: impl CloudClientInterface) -> Result<()> {
        let list = client
            .get_databases(None)
            .await
            .context("Problem fetching databases")?;
        if list.iter().any(|d| d.name == self.name) {
            anyhow::bail!(r#"Database "{}" already exists"#, self.name)
        }
        client
            .create_database(self.name.clone(), None)
            .await
            .with_context(|| format!("Problem creating database {}", self.name))?;
        println!("Database \"{}\" created", self.name);
        Ok(())
    }
}

impl DeleteCommand {
    pub async fn run(self, client: impl CloudClientInterface) -> Result<()> {
        let list = client
            .get_databases(None)
            .await
            .context("Problem fetching databases")?;
        let found = list.iter().find(|d| d.name == self.name);
        match found {
            None => anyhow::bail!("No database found with name \"{}\"", self.name),
            Some(db) => {
                // TODO: Fail if apps exist that are currently using a database
                if self.yes
                    || prompt_delete_resource(&self.name, &db.links, ResourceType::Database)?
                {
                    client
                        .delete_database(self.name.clone())
                        .await
                        .with_context(|| format!("Problem deleting database {}", self.name))?;
                    println!("Database \"{}\" deleted", self.name);
                }
            }
        }
        Ok(())
    }
}

impl ExecuteCommand {
    pub async fn run(self, client: impl CloudClientInterface) -> Result<()> {
        let target = ResourceTarget::from_inputs(&self.database, &self.label, &self.app)?;
        let list = client
            .get_databases(None)
            .await
            .context("Problem fetching databases")?;
        let database = target
            .find_in(to_resource_links(list), ResourceType::Database)?
            .name;
        let statement = if let Some(path) = self.statement.strip_prefix('@') {
            std::fs::read_to_string(path)
                .with_context(|| format!("could not read sql file at '{path}'"))?
        } else {
            self.statement
        };
        client
            .execute_sql(database, statement)
            .await
            .context("Problem executing SQL")?;
        Ok(())
    }
}

impl ListCommand {
    pub async fn run(self) -> Result<()> {
        if let (ListFormat::Json, Some(_)) = (&self.format, self.group_by) {
            bail!("Grouping is not supported with JSON format output")
        }

        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let mut databases = client
            .get_databases(None)
            .await
            .context("Problem listing databases")?;

        if databases.is_empty() {
            println!("No databases");
            return Ok(());
        }
        if let Some(name) = &self.database {
            databases.retain(|db| db.name == *name);
            if databases.is_empty() {
                println!("No database with name '{name}'");
                return Ok(());
            }
        }

        let resource_links = to_resource_links(databases);
        match self.format {
            ListFormat::Json => {
                print_json(resource_links, self.app.as_deref(), ResourceType::Database)
            }
            ListFormat::Table => print_table(
                resource_links,
                self.app.as_deref(),
                self.group_by.map(Into::into),
                ResourceType::Database,
            ),
        }
    }
}

fn to_resource_links(databases: Vec<Database>) -> Vec<ResourceLinks> {
    databases
        .into_iter()
        .map(|db| ResourceLinks::new(db.name, db.links))
        .collect()
}

impl RenameCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = client
            .get_databases(None)
            .await
            .context("Problem fetching databases")?;
        let found = list.iter().find(|d| d.name == self.name);
        if found.is_none() {
            anyhow::bail!("No database found with name \"{}\"", self.name);
        }
        client
            .rename_database(self.name.clone(), self.new_name.clone())
            .await?;
        println!(
            "Database \"{}\" is now named \"{}\"",
            self.name, self.new_name
        );
        Ok(())
    }
}

pub fn database_has_link(database: &Database, label: &str, app: Option<&str>) -> bool {
    database
        .links
        .iter()
        .any(|l| l.label == label && l.app_name.as_deref() == app)
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use cloud::MockCloudClientInterface;
    use cloud_openapi::models::ResourceLabel;

    #[tokio::test]
    async fn test_create_if_db_already_exists_then_error() -> Result<()> {
        let command = CreateCommand {
            name: "db1".to_string(),
            common: Default::default(),
        };
        let dbs = vec![
            Database::new("db1".to_string(), vec![]),
            Database::new("db2".to_string(), vec![]),
        ];

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().return_once(move |_| Ok(dbs));

        let result = command.run(mock).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"Database "db1" already exists"#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_create_if_db_does_not_exist_db_is_created() -> Result<()> {
        let command = CreateCommand {
            name: "db1".to_string(),
            common: Default::default(),
        };
        let dbs = vec![Database::new("db2".to_string(), vec![])];

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().return_once(move |_| Ok(dbs));
        mock.expect_create_database()
            .withf(move |db, rl| db == "db1" && rl.is_none())
            .returning(|_, _| Ok(()));

        command.run(mock).await
    }

    #[tokio::test]
    async fn test_delete_if_db_does_not_exist_then_error() -> Result<()> {
        let command = DeleteCommand {
            name: "db1".to_string(),
            common: Default::default(),
            yes: true,
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().returning(move |_| Ok(vec![]));

        let result = command.run(mock).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"No database found with name "db1""#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_if_db_exists_then_it_is_deleted() -> Result<()> {
        let command = DeleteCommand {
            name: "db1".to_string(),
            common: Default::default(),
            yes: true,
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases()
            .returning(move |_| Ok(vec![Database::new("db1".to_string(), vec![])]));
        mock.expect_delete_database().returning(|_| Ok(()));

        command.run(mock).await
    }

    #[tokio::test]
    async fn test_execute_by_db_if_db_exists_then_statement_is_executed() -> Result<()> {
        let db = "db1";
        let sql = "CREATE TABLE test (message TEXT)";

        let command = ExecuteCommand {
            database: Some(db.to_string()),
            label: None,
            app: None,
            common: Default::default(),
            statement: sql.to_owned(),
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases()
            .returning(move |_| Ok(vec![Database::new(db.to_string(), vec![])]));
        mock.expect_execute_sql()
            .withf(move |dbarg, sqlarg| dbarg == db && sqlarg == sql)
            .returning(|_, _| Ok(()));

        command.run(mock).await
    }

    #[tokio::test]
    async fn test_execute_by_db_if_db_does_not_exist_then_error() -> Result<()> {
        let askeddb = "asked-for";
        let actualdb = "actual";
        let sql = "CREATE TABLE test (message TEXT)";

        let command = ExecuteCommand {
            database: Some(askeddb.to_string()),
            label: None,
            app: None,
            common: Default::default(),
            statement: sql.to_owned(),
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases()
            .returning(move |_| Ok(vec![Database::new(actualdb.to_string(), vec![])]));

        let err = command
            .run(mock)
            .await
            .expect_err("exec should have errored but did not");
        assert_eq!(
            err.to_string(),
            r#"No database found with name "asked-for""#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_execute_by_label_if_label_linked_then_statement_is_executed() -> Result<()> {
        let label = "email";
        let app = "messaging";
        let sql = "CREATE TABLE test (message TEXT)";

        let command = ExecuteCommand {
            database: None,
            label: Some(label.to_string()),
            app: Some(app.to_string()),
            common: Default::default(),
            statement: sql.to_owned(),
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases()
            .returning(move |_| Ok(fake_dbs()));
        mock.expect_execute_sql()
            .withf(move |dbarg, sqlarg| dbarg == "db2" && sqlarg == sql)
            .returning(|_, _| Ok(()));

        command.run(mock).await
    }

    #[tokio::test]
    async fn test_execute_by_label_if_label_not_linked_then_error() -> Result<()> {
        let label = "snailmail";
        let app = "messaging";
        let sql = "CREATE TABLE test (message TEXT)";

        let command = ExecuteCommand {
            database: None,
            label: Some(label.to_string()),
            app: Some(app.to_string()),
            common: Default::default(),
            statement: sql.to_owned(),
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases()
            .returning(move |_| Ok(fake_dbs()));

        let err = command
            .run(mock)
            .await
            .expect_err("exec should have errored but did not");
        assert_eq!(
            err.to_string(),
            r#"No database found with label "snailmail" for app "messaging""#
        );
        Ok(())
    }

    fn fake_dbs() -> Vec<Database> {
        vec![
            Database::new(
                "db1".to_string(),
                vec![
                    resource_label("voicemail", "messaging"),
                    resource_label("email", "attachment-manager"),
                ],
            ),
            Database::new(
                "db2".to_string(),
                vec![
                    resource_label("notes", "docs"),
                    resource_label("email", "messaging"),
                ],
            ),
        ]
    }

    fn resource_label(label: &str, app: &str) -> ResourceLabel {
        ResourceLabel {
            label: label.to_owned(),
            app_id: uuid::Uuid::new_v4(),
            app_name: Some(app.to_owned()),
        }
    }
}
