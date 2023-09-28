use crate::commands::create_cloud_client;
use crate::commands::link::Link;
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
use cloud_openapi::models::Database;
use cloud_openapi::models::ResourceLabel;
use dialoguer::Input;
use std::collections::HashMap;

/// Manage Fermyon Cloud NoOps SQL databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQLite databases")]
pub enum SqliteCommand {
    /// Create a NoOps SQL database
    Create(CreateCommand),
    /// Delete a NoOps SQL database
    Delete(DeleteCommand),
    /// Execute SQLite statements against a NoOps SQL database
    Execute(ExecuteCommand),
    /// List all NoOps SQL databases of a user
    List(ListCommand),
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
    #[clap(short = 'a', long = "app")]
    app: Option<String>,
    #[clap(short = 'd', long = "database")]
    database: Option<String>,
    // TODO: like templates, enable multiple list formats
    // #[clap(value_enum, long = "format", default_value = "table", hide = true)]
    // pub format: ListFormat,
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
            Self::Create(cmd) => cmd.run().await,
            Self::Delete(cmd) => cmd.run().await,
            Self::Execute(cmd) => cmd.run().await,
            Self::List(cmd) => cmd.run().await,
        }
    }
}

impl CreateCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem fetching databases")?;
        if list.iter().any(|d| d.name == self.name) {
            anyhow::bail!("Database {} already exists", self.name)
        }
        CloudClient::create_database(&client, self.name.clone(), None)
            .await
            .with_context(|| format!("Problem creating database {}", self.name))?;
        println!("Database \"{}\" created", self.name);
        Ok(())
    }
}

impl DeleteCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem fetching databases")?;
        let found = list.iter().find(|d| d.name == self.name);
        match found {
            None => anyhow::bail!("No database found with name \"{}\"", self.name),
            Some(db) => {
                // TODO: Fail if apps exist that are currently using a database
                if self.yes || prompt_delete_database(&self.name, &db.links)? {
                    CloudClient::delete_database(&client, self.name.clone())
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
        let list = CloudClient::get_databases(&client, None)
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
        CloudClient::execute_sql(&client, self.name, statement)
            .await
            .context("Problem executing SQL")?;
        Ok(())
    }
}

impl ListCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem listing databases")?;
        print_databases(list, self.app, self.database);
        Ok(())
    }
}

fn print_databases(mut databases: Vec<Database>, app: Option<String>, database: Option<String>) {
    if databases.is_empty() {
        println!("No databases");
        return;
    }
    if let Some(name) = &database {
        databases.retain(|db| db.name == *name);
    }

    let no_link_dbs: Vec<_> = databases.iter().filter(|db| db.links.is_empty()).collect();
    let mut links: Vec<Link> = databases
        .iter()
        .flat_map(|db| {
            db.links.iter().map(|l| Link {
                resource: db.name.clone(),
                resource_label: l.clone(),
            })
        })
        .collect();
    if let Some(name) = &app {
        links.retain(|d| d.app_name() == *name);
    }

    let mut table = comfy_table::Table::new();
    let header: Vec<&str> = vec!["Database", "Link"];
    table.set_header(header);
    no_link_dbs.into_iter().for_each(|db| {
        table.add_row(vec![db.name.clone(), String::from("-")]);
    });

    if app.is_none() && database.is_none() {
        let mut map = HashMap::new();
        links.into_iter().for_each(|d| {
            map.entry(d.resource.clone())
                .and_modify(|v| *v = format!("{}, {}:{}", *v, d.app_name(), d.resource_label.label))
                .or_insert(format!("{}:{}", d.app_name(), d.resource_label.label));
        });
        map.into_iter().for_each(|e| {
            table.add_row(vec![e.0, e.1]);
        })
    } else {
        links.into_iter().for_each(|d| {
            table.add_row(vec![
                d.resource.clone(),
                format!("{}:{}", d.app_name(), d.resource_label.label),
            ]);
        });
    }
    println!("{table}");
}

fn prompt_delete_database(database: &str, links: &[ResourceLabel]) -> std::io::Result<bool> {
    let app_name_unwrapping = |r: &ResourceLabel| {
        r.app_name
            .clone()
            .expect("no app name field in ResourceLabel")
            .expect("no app name set in ResourceLabel")
    };
    let existing_links = links
        .iter()
        .map(|l| format!("{}:{}", app_name_unwrapping(l), l.label))
        .collect::<Vec<String>>()
        .join(", ");
    let mut prompt = String::new();
    if !existing_links.is_empty() {
        // TODO: use warning color text
        prompt.push_str(&format!("Database \"{database}\" is currently linked to the following apps: {existing_links}.\n It is recommended to use `spin cloud link sqlite `link a new database to the apps before deleting."))
    }
    let mut input = Input::<String>::new();
    prompt.push_str(&format!(
        "The action is irreversible. Please type \"{database}\" for confirmation"
    ));
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
