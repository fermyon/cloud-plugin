use std::collections::HashMap;

use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
// use cloud_openapi::models::Database;
use cloud::mocks::Database as MockDatabase;
use cloud::mocks::Link;
use dialoguer::Input;

use crate::commands::create_cloud_client;
use crate::opts::*;

/// Manage Fermyon Cloud SQLite databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQLite databases")]
pub enum SqliteCommand {
    /// Delete a SQLite database
    Delete(DeleteCommand),
    /// Execute SQL against a SQLite database
    Execute(ExecuteCommand),
    /// List all SQLite databases of a user
    List(ListCommand),
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
            Self::Delete(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem fetching databases")?;
                if !list.iter().any(|d| d.name == cmd.name) {
                    anyhow::bail!("No database found with name \"{}\"", cmd.name);
                }
                // TODO: Fail if apps exist that are currently using a database
                if cmd.yes || prompt_delete_database(&cmd.name)? {
                    CloudClient::delete_database(&client, cmd.name.clone())
                        .await
                        .with_context(|| format!("Problem deleting database {}", cmd.name))?;
                    println!("Database \"{}\" deleted", cmd.name);
                }
            }
            Self::Execute(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem fetching databases")?;
                if !list.iter().any(|d| d.name == cmd.name) {
                    anyhow::bail!("No database found with name \"{}\"", cmd.name);
                }
                let statement = if let Some(path) = cmd.statement.strip_prefix('@') {
                    std::fs::read_to_string(path)
                        .with_context(|| format!("could not read sql file at '{path}'"))?
                } else {
                    cmd.statement
                };
                CloudClient::execute_sql(&client, cmd.name, statement)
                    .await
                    .context("Problem executing SQL")?;
            }
            Self::List(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem listing databases")?;
                print_databases(list, cmd.app, cmd.database);
            }
        }
        Ok(())
    }
}

fn print_databases(
    mut databases: Vec<MockDatabase>,
    app: Option<String>,
    database: Option<String>,
) {
    if databases.is_empty() {
        println!("No databases");
        return;
    }
    if let Some(name) = &database {
        databases.retain(|db| db.name == *name);
    }
    let no_link_dbs: Vec<_> = databases.iter().filter(|db| db.links.is_empty()).collect();
    let mut links: Vec<Link> = databases.iter().flat_map(|db| db.links.clone()).collect();
    if let Some(name) = &app {
        links.retain(|l| l.app == *name);
    }

    let mut table = comfy_table::Table::new();
    let header: Vec<&str> = vec!["Database", "Link"];
    table.set_header(header);
    no_link_dbs.into_iter().for_each(|db| {
        table.add_row(vec![db.name.clone(), String::from("-")]);
    });

    if app.is_none() && database.is_none() {
        let mut map = HashMap::new();
        links.into_iter().for_each(|l| {
            map.entry(l.database)
                .and_modify(|v| *v = format!("{}, {}:{}", *v, l.app, l.name))
                .or_insert(format!("{}:{}", l.app, l.name));
        });
        map.into_iter().for_each(|e| {
            table.add_row(vec![e.0, e.1]);
        })
    } else {
        links.into_iter().for_each(|l| {
            table.add_row(vec![l.database.clone(), format!("{}:{}", l.app, l.name)]);
        });
    }
    println!("{table}");
}

fn prompt_delete_database(database_name: &str) -> std::io::Result<bool> {
    let mut input = Input::<String>::new();
    let prompt =
        format!("The action is irreversible. Please type \"{database_name}\" for confirmation",);
    input.with_prompt(prompt);
    let answer = input.interact_text()?;
    if answer != database_name {
        println!("Invalid confirmation. Will not delete database.");
        Ok(false)
    } else {
        println!("Deleting database ...");
        Ok(true)
    }
}
