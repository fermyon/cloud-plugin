use anyhow::{bail, Context, Result};
use cloud::CloudClientInterface;
use cloud_openapi::models::{Database, ResourceLabel};

use crate::commands::sqlite::database_has_link;
use crate::random_name::RandomNameGenerator;
use std::collections::HashSet;
use uuid::Uuid;

use super::interaction::Interactor;

// Loops through an app's manifest and creates databases.
// Returns a list of database and label pairs that should be
// linked to the app once it is created.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_databases_for_new_app(
    interactor: &impl Interactor,
    client: &impl CloudClientInterface,
    app_name: &str,
    labels: HashSet<String>,
) -> anyhow::Result<Option<Vec<(String, String)>>> {
    let mut databases_to_link = Vec::new();
    for label in labels {
        let db =
            match get_database_selection_for_new_app(interactor, app_name, client, &label).await? {
                DatabaseSelection::Existing(db) => db,
                DatabaseSelection::New(db) => {
                    client.create_database(db.clone(), None).await?;
                    db
                }
                // User canceled terminal interaction
                DatabaseSelection::Cancelled => return Ok(None),
            };
        databases_to_link.push((db, label));
    }
    Ok(Some(databases_to_link))
}

// Loops through an updated app's manifest and creates and links any newly referenced databases.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_and_link_databases_for_existing_app(
    interactor: &impl Interactor,
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    labels: HashSet<String>,
) -> anyhow::Result<Option<()>> {
    for label in labels {
        let resource_label = ResourceLabel {
            app_id,
            label,
            app_name: Some(app_name.to_string()),
        };
        if let ExistingAppDatabaseSelection::NotYetLinked(selection) =
            get_database_selection_for_existing_app(interactor, app_name, client, &resource_label)
                .await?
        {
            match selection {
                // User canceled terminal interaction
                DatabaseSelection::Cancelled => return Ok(None),
                DatabaseSelection::New(db) => {
                    client.create_database(db, Some(resource_label)).await?;
                }
                DatabaseSelection::Existing(db) => {
                    client
                        .create_database_link(&db, resource_label)
                        .await
                        .with_context(|| {
                            format!(r#"Could not link database "{}" to app "{}""#, db, app_name,)
                        })?;
                }
            }
        }
    }
    Ok(Some(()))
}

pub(super) async fn link_databases(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    database_labels: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (database, label) in database_labels {
        let resource_label = ResourceLabel {
            label,
            app_id,
            app_name: Some(app_name.to_owned()),
        };
        client
            .create_database_link(&database, resource_label)
            .await
            .with_context(|| {
                format!(
                    r#"Failed to link database "{}" to app "{}""#,
                    database, app_name
                )
            })?;
    }
    Ok(())
}

/// A user's selection of a database to link to a label
enum DatabaseSelection {
    Existing(String),
    New(String),
    Cancelled,
}

/// Whether a database has already been linked or not
enum ExistingAppDatabaseSelection {
    NotYetLinked(DatabaseSelection),
    AlreadyLinked,
}

async fn get_database_selection_for_existing_app(
    interactor: &impl Interactor,
    name: &str,
    client: &impl CloudClientInterface,
    resource_label: &ResourceLabel,
) -> Result<ExistingAppDatabaseSelection> {
    let databases = client.get_databases(None).await?;
    if databases
        .iter()
        .any(|d| database_has_link(d, &resource_label.label, resource_label.app_name.as_deref()))
    {
        return Ok(ExistingAppDatabaseSelection::AlreadyLinked);
    }
    let selection = prompt_database_selection(interactor, name, &resource_label.label, databases)?;
    Ok(ExistingAppDatabaseSelection::NotYetLinked(selection))
}

async fn get_database_selection_for_new_app(
    interactor: &impl Interactor,
    name: &str,
    client: &impl CloudClientInterface,
    label: &str,
) -> Result<DatabaseSelection> {
    let databases = client.get_databases(None).await?;
    prompt_database_selection(interactor, name, label, databases)
}

fn prompt_database_selection(
    interactor: &impl Interactor,
    name: &str,
    label: &str,
    databases: Vec<Database>,
) -> Result<DatabaseSelection> {
    let prompt = format!(
        r#"App "{name}" accesses a database labeled "{label}"
Would you like to link an existing database or create a new database?"#
    );
    let existing_opt = "Use an existing database and link app to it";
    let create_opt = "Create a new database and link the app to it";
    let opts = vec![existing_opt, create_opt];
    let index = match interactor.select(&prompt, &opts, 1)? {
        Some(i) => i,
        None => return Ok(DatabaseSelection::Cancelled),
    };
    match index {
        0 => prompt_for_existing_database(
            interactor,
            name,
            label,
            databases.into_iter().map(|d| d.name).collect::<Vec<_>>(),
        ),
        1 => prompt_link_to_new_database(
            interactor,
            name,
            label,
            databases
                .iter()
                .map(|d| d.name.as_str())
                .collect::<HashSet<_>>(),
        ),
        _ => bail!("Choose unavailable option"),
    }
}

fn prompt_for_existing_database(
    interactor: &impl Interactor,
    name: &str,
    label: &str,
    mut database_names: Vec<String>,
) -> Result<DatabaseSelection> {
    let prompt =
        format!(r#"Which database would you like to link to {name} using the label "{label}""#);
    let index = match interactor.select(&prompt, &database_names, 0)? {
        Some(i) => i,
        None => return Ok(DatabaseSelection::Cancelled),
    };
    Ok(DatabaseSelection::Existing(database_names.remove(index)))
}

fn prompt_link_to_new_database(
    interactor: &impl Interactor,
    name: &str,
    label: &str,
    existing_names: HashSet<&str>,
) -> Result<DatabaseSelection> {
    let generator = RandomNameGenerator::new();
    let default_name = generator
        .generate_unique(existing_names, 20)
        .context("could not generate unique database name")?;

    let prompt = format!(
        r#"What would you like to name your database?
Note: This name is used when managing your database at the account level. The app "{name}" will refer to this database by the label "{label}".
Other apps can use different labels to refer to the same database."#
    );
    let name = interactor.input(&prompt, &default_name)?;
    Ok(DatabaseSelection::New(name))
}
