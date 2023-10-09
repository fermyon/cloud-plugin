use crate::commands::create_cloud_client;
use crate::commands::link::Link;
use crate::opts::*;
use anyhow::bail;
use anyhow::{Context, Result};
use clap::{Args, Parser, ValueEnum};
use cloud::CloudClientInterface;
use cloud_openapi::models::Database;
use cloud_openapi::models::ResourceLabel;
use comfy_table::presets::ASCII_BORDERS_ONLY_CONDENSED;
use dialoguer::Input;
use serde::Serialize;
use std::collections::BTreeMap;
use std::str::FromStr;

/// Manage Fermyon Cloud NoOps SQL databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud NoOps SQL databases")]
pub enum SqliteCommand {
    /// Create a NoOps SQL database
    Create(CreateCommand),
    /// Delete a NoOps SQL database
    Delete(DeleteCommand),
    /// Execute SQLite statements against a NoOps SQL database
    Execute(ExecuteCommand),
    /// List all NoOps SQL databases of a user
    List(ListCommand),
    /// Rename a NoOps SQL database
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
    #[clap(value_parser = clap::builder::ValueParser::new(disallow_empty))]
    name: String,

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

fn disallow_empty(statement: &str) -> anyhow::Result<String> {
    if statement.trim().is_empty() {
        anyhow::bail!("cannot be empty");
    }
    return Ok(statement.trim().to_owned());
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
            "database" => Ok(Self::App),
            s => Err(format!("Unrecognized group-by option: '{s}'")),
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ListFormat {
    Table,
    Json,
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
            Self::Execute(cmd) => cmd.run().await,
            Self::List(cmd) => cmd.run().await,
            Self::Rename(cmd) => cmd.run().await,
        }
    }
}

impl CreateCommand {
    pub async fn run(self, client: impl CloudClientInterface) -> Result<()> {
        let list: Vec<Database> = client
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
                if self.yes || prompt_delete_database(&self.name, &db.links)? {
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
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = client
            .get_databases(None)
            .await
            .context("Problem fetching databases")?;
        if !list.iter().any(|d| d.name == self.name) {
            anyhow::bail!("No database found with name \"{}\"", self.name);
        }
        let statement = if let Some(path) = self.statement.strip_prefix('@') {
            std::fs::read_to_string(path)
                .with_context(|| format!("could not read sql file at '{path}'"))?
        } else {
            self.statement
        };
        client
            .execute_sql(self.name, statement)
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

        match self.format {
            ListFormat::Json => self.print_json(databases),
            ListFormat::Table => self.print_table(databases),
        }
    }

    fn print_json(&self, mut databases: Vec<Database>) -> Result<()> {
        if let Some(app) = &self.app {
            databases.retain(|d| {
                d.links
                    .iter()
                    .any(|l| l.app_name.as_deref().unwrap_or("UNKNOWN") == app)
            });
        }
        let json_vals: Vec<_> = databases.iter().map(json_list_format).collect();
        let json_text = serde_json::to_string_pretty(&json_vals)?;
        println!("{}", json_text);
        Ok(())
    }

    fn print_table(&self, databases: Vec<Database>) -> Result<()> {
        let databases_without_links = databases.iter().filter(|db| db.links.is_empty());

        let mut links = databases
            .iter()
            .flat_map(|db| {
                db.links.iter().map(|l| Link {
                    resource: db.name.clone(),
                    resource_label: l.clone(),
                })
            })
            .collect::<Vec<_>>();
        if let Some(name) = &self.app {
            links.retain(|l| l.app_name() == *name);
            if links.is_empty() {
                println!("No databases linked to an app named '{name}'");
                return Ok(());
            }
        }
        match self.group_by.unwrap_or_default() {
            GroupBy::App => print_apps(links, databases_without_links),
            GroupBy::Database => print_databases(links, databases_without_links),
        }
        Ok(())
    }
}

fn json_list_format(database: &Database) -> DatabasesListJson<'_> {
    DatabasesListJson {
        database: &database.name,
        links: database
            .links
            .iter()
            .map(|l| ResourceLabelJson {
                label: &l.label,
                app: l.app_name.as_deref().unwrap_or("UNKNOWN"),
            })
            .collect(),
    }
}

#[derive(Serialize)]
struct DatabasesListJson<'a> {
    database: &'a str,
    links: Vec<ResourceLabelJson<'a>>,
}

/// A ResourceLabel type without app ID for JSON output
#[derive(Serialize)]
struct ResourceLabelJson<'a> {
    label: &'a str,
    app: &'a str,
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

/// Print apps optionally filtering to a specifically supplied app and/or database
fn print_apps<'a>(
    mut links: Vec<Link>,
    databases_without_links: impl Iterator<Item = &'a Database>,
) {
    links.sort_by(|l1, l2| l1.app_name().cmp(l2.app_name()));

    let mut table = comfy_table::Table::new();
    table.load_preset(ASCII_BORDERS_ONLY_CONDENSED);
    table.set_header(vec!["App", "Label", "Database"]);

    let rows = links.iter().map(|link| {
        [
            link.app_name(),
            link.resource_label.label.as_str(),
            link.resource.as_str(),
        ]
    });
    table.add_rows(rows);
    println!("{table}");

    let mut databases_without_links = databases_without_links.peekable();
    if databases_without_links.peek().is_none() {
        return;
    }

    let mut table = comfy_table::Table::new();
    println!("Databases not linked to any app");
    table.set_header(vec!["Database"]);
    table.add_rows(databases_without_links.map(|d| [&d.name]));
    println!("{table}");
}

/// Print databases optionally filtering to a specifically supplied app and/or database
fn print_databases<'a>(
    mut links: Vec<Link>,
    databases_without_links: impl Iterator<Item = &'a Database>,
) {
    links.sort_by(|l1, l2| l1.resource.cmp(&l2.resource));

    let mut table = comfy_table::Table::new();
    table.load_preset(ASCII_BORDERS_ONLY_CONDENSED);
    table.set_header(vec!["Database", "Links"]);
    table.add_rows(databases_without_links.map(|d| [&d.name, "-"]));

    let mut map = BTreeMap::new();
    for link in &links {
        let app_name = link.app_name();
        map.entry(&link.resource)
            .and_modify(|v| *v = format!("{}, {}:{}", *v, app_name, link.resource_label.label))
            .or_insert(format!("{}:{}", app_name, link.resource_label.label));
    }
    table.add_rows(map.iter().map(|(d, l)| [d, l]));
    println!("{table}");
}

fn prompt_delete_database(database: &str, links: &[ResourceLabel]) -> std::io::Result<bool> {
    let existing_links = links
        .iter()
        .map(|l| l.app_name.as_deref().unwrap_or("UNKNOWN"))
        .collect::<Vec<&str>>()
        .join(", ");
    let mut prompt = String::new();
    if !existing_links.is_empty() {
        // TODO: use warning color text
        prompt.push_str(&format!("Database \"{database}\" is currently linked to the following apps: {existing_links}.\n\
        It is recommended to use `spin cloud link sqlite` to link to another database to those apps before deleting.\n"))
    }
    prompt.push_str(&format!(
        "The action is irreversible. Please type \"{database}\" for confirmation"
    ));
    let mut input = Input::<String>::new();
    input.with_prompt(prompt);
    let answer = input.interact_text()?;
    if answer != database {
        println!("Invalid confirmation. Will not delete database.");
        Ok(false)
    } else {
        println!("Deleting database ...");
        Ok(true)
    }
}

pub fn find_database_link(db: &Database, label: &str) -> Option<Link> {
    db.links.iter().find_map(|r| {
        if r.label == label {
            Some(Link::new(r.clone(), db.name.clone()))
        } else {
            None
        }
    })
}

pub fn database_has_link(db: &Database, link: &ResourceLabel) -> bool {
    db.links.iter().any(|l| l == link)
}

#[cfg(test)]
mod sqlite_tests {
    use super::*;
    use cloud::MockCloudClientInterface;

    #[tokio::test]
    async fn test_create_error_already_exists() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = CreateCommand {
            name: "db1".to_string(),
            common: Default::default(),
        };
        let dbs = vec![
            Database::new("db1".to_string(), vec![]),
            Database::new("db2".to_string(), vec![]),
        ];
        mock.expect_get_databases().return_once(move |_| Ok(dbs));

        let result = command.run(mock).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"Database "db1" already exists"#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_create_success() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = CreateCommand {
            name: "db1".to_string(),
            common: Default::default(),
        };
        let dbs = vec![Database::new("db2".to_string(), vec![])];
        mock.expect_get_databases().return_once(move |_| Ok(dbs));
        mock.expect_create_database()
            .withf(move |db, rl| db == "db1" && rl.is_none())
            .returning(|_, _| Ok(()));
        command.run(mock).await
    }

    #[tokio::test]
    async fn test_delete_error_does_not_exist() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = DeleteCommand {
            name: "db1".to_string(),
            common: Default::default(),
            yes: true,
        };
        mock.expect_get_databases().returning(move |_| Ok(vec![]));

        let result = command.run(mock).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"No database found with name "db1""#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_delete_success() -> Result<()> {
        let mut mock = MockCloudClientInterface::new();
        let command = DeleteCommand {
            name: "db1".to_string(),
            common: Default::default(),
            yes: true,
        };
        mock.expect_get_databases()
            .returning(move |_| Ok(vec![Database::new("db1".to_string(), vec![])]));
        mock.expect_delete_database().returning(|_| Ok(()));
        command.run(mock).await
    }
}
