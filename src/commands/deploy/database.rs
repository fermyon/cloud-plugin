// TODO(kate): rename this module to something more generic like `resource`
use anyhow::{anyhow, bail, Context, Result};
use cloud::CloudClientInterface;
use cloud_openapi::models::ResourceLabel;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

use crate::commands::links_output::ResourceLinks;
use crate::commands::links_output::ResourceType;
use crate::random_name::RandomNameGenerator;

/// A user's selection of a database to link to a label
pub(super) enum ResourceSelection {
    Existing(String),
    New(String),
    Cancelled,
}

/// Whether a database has already been linked or not
enum ExistingAppDatabaseSelection {
    NotYetLinked(ResourceSelection),
    AlreadyLinked,
}

async fn get_resources(
    client: &impl CloudClientInterface,
    resource_type: ResourceType,
) -> Result<Vec<ResourceLinks>> {
    match resource_type {
        ResourceType::Database => Ok(client
            .get_databases(None)
            .await?
            .into_iter()
            .map(|r| ResourceLinks::new(r.name, r.links))
            .collect()),
        ResourceType::KeyValueStore => Ok(client
            .get_key_value_stores(None)
            .await?
            .into_iter()
            .map(|r| ResourceLinks::new(r.name, r.links))
            .collect()),
    }
}

async fn get_resource_selection_for_existing_app(
    name: &str,
    client: &impl CloudClientInterface,
    resource_label: &ResourceLabel,
    interact: &dyn InteractionStrategy,
    resource_type: ResourceType,
) -> Result<ExistingAppDatabaseSelection> {
    let resources = get_resources(client, resource_type).await?;
    if resources
        .iter()
        .any(|d| d.has_link(&resource_label.label, resource_label.app_name.as_deref()))
    {
        return Ok(ExistingAppDatabaseSelection::AlreadyLinked);
    }
    let selection = interact.prompt_resource_selection(
        name,
        &resource_label.label,
        resources,
        resource_type,
    )?;
    Ok(ExistingAppDatabaseSelection::NotYetLinked(selection))
}

async fn get_resource_selection_for_new_app(
    name: &str,
    client: &impl CloudClientInterface,
    label: &str,
    interact: &dyn InteractionStrategy,
    resource_type: ResourceType,
) -> Result<ResourceSelection> {
    let resources = get_resources(client, resource_type).await?;
    interact.prompt_resource_selection(name, label, resources, resource_type)
}

pub(super) struct Interactive;

pub(super) trait InteractionStrategy {
    fn prompt_resource_selection(
        &self,
        name: &str,
        label: &str,
        resources: Vec<ResourceLinks>,
        resource_type: ResourceType,
    ) -> Result<ResourceSelection>;
}

impl InteractionStrategy for Interactive {
    fn prompt_resource_selection(
        &self,
        name: &str,
        label: &str,
        resources: Vec<ResourceLinks>,
        resource_type: ResourceType,
    ) -> Result<ResourceSelection> {
        let prompt = format!(
            r#"App "{name}" accesses a {resource_type} labeled "{label}"
    Would you like to link an existing {resource_type} or create a new {resource_type}?"#
        );
        let existing_opt = format!("Use an existing {resource_type} and link app to it");
        let create_opt = format!("Create a new {resource_type} and link the app to it");
        let opts = vec![existing_opt, create_opt];
        let index = match dialoguer::Select::new()
            .with_prompt(prompt)
            .items(&opts)
            .default(1)
            .interact_opt()?
        {
            Some(i) => i,
            None => return Ok(ResourceSelection::Cancelled),
        };
        match index {
            0 => self.prompt_for_existing_resource(
                name,
                label,
                resources.into_iter().map(|d| d.name).collect::<Vec<_>>(),
                resource_type,
            ),
            1 => self.prompt_link_to_new_resource(
                name,
                label,
                resources
                    .iter()
                    .map(|d| d.name.as_str())
                    .collect::<HashSet<_>>(),
                ResourceType::Database,
            ),
            _ => bail!("Choose unavailable option"),
        }
    }
}

const NAME_GENERATION_MAX_ATTEMPTS: usize = 100;

impl Interactive {
    fn prompt_for_existing_resource(
        &self,
        name: &str,
        label: &str,
        mut resource_names: Vec<String>,
        resource_type: ResourceType,
    ) -> Result<ResourceSelection> {
        let prompt = format!(
            r#"Which {resource_type} would you like to link to {name} using the label "{label}""#
        );
        let index = match dialoguer::Select::new()
            .with_prompt(prompt)
            .items(&resource_names)
            .default(0)
            .interact_opt()?
        {
            Some(i) => i,
            None => return Ok(ResourceSelection::Cancelled),
        };
        Ok(ResourceSelection::Existing(resource_names.remove(index)))
    }

    fn prompt_link_to_new_resource(
        &self,
        name: &str,
        label: &str,
        existing_names: HashSet<&str>,
        resource_type: ResourceType,
    ) -> Result<ResourceSelection> {
        let generator = RandomNameGenerator::new();
        let default_name = generator
            .generate_unique(existing_names, NAME_GENERATION_MAX_ATTEMPTS)
            .context("could not generate unique name")?;

        let prompt = format!(
            r#"What would you like to name your {resource_type}?
    Note: This name is used when managing your {resource_type} at the account level. The app "{name}" will refer to this {resource_type} by the label "{label}".
    Other apps can use different labels to refer to the same {resource_type}."#
        );
        let name = dialoguer::Input::new()
            .with_prompt(prompt)
            .default(default_name)
            .interact_text()?;
        Ok(ResourceSelection::New(name))
    }
}

#[derive(Default)]
pub(super) struct Scripted {
    labels_to_dbs: HashMap<String, DatabaseRef>,
}

impl Scripted {
    pub(super) fn set_label_action(&mut self, label: &str, db: DatabaseRef) -> anyhow::Result<()> {
        match self.labels_to_dbs.entry(label.to_owned()) {
            Entry::Occupied(_) => bail!("Label {label} is linked more than once"),
            Entry::Vacant(e) => e.insert(db),
        };
        println!("Labels are {:?}", self.labels_to_dbs);
        Ok(())
    }
}

// Using an enum to allow for future "any other db label" linking
#[derive(Clone, Debug, Default)]
pub(super) enum DefaultLabelAction {
    #[default]
    Reject,
}

// Using an enum to allow for future "create new and link that" linking
#[derive(Clone, Debug)]
pub(super) enum DatabaseRef {
    Named(String),
}

impl InteractionStrategy for Scripted {
    fn prompt_resource_selection(
        &self,
        _name: &str,
        label: &str,
        resources: Vec<ResourceLinks>,
        _resource_type: ResourceType,
    ) -> Result<ResourceSelection> {
        let existing_names: HashSet<&str> = resources.iter().map(|db| db.name.as_str()).collect();
        let requested_db = self.db_ref_for(label)?;
        match requested_db {
            DatabaseRef::Named(requested_db) => {
                let name = requested_db.to_owned();
                if existing_names.contains(name.as_str()) {
                    Ok(ResourceSelection::Existing(name))
                } else {
                    Ok(ResourceSelection::New(name))
                }
            }
        }
    }
}

impl Scripted {
    fn db_ref_for(&self, label: &str) -> anyhow::Result<&DatabaseRef> {
        match self.labels_to_dbs.get(label) {
            Some(db_ref) => Ok(db_ref),
            None => Err(anyhow!("No link specified for label '{label}'")),
        }
    }
}

// Loops through an app's manifest and creates resources.
// Returns a list of resource and label pairs that should be
// linked to the app once it is created.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_resources_for_new_app(
    client: &impl CloudClientInterface,
    name: &str,
    labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
    resource_type: ResourceType,
) -> anyhow::Result<Option<Vec<(String, String)>>> {
    let mut resources_to_link = Vec::new();
    for label in labels {
        let r =
            match get_resource_selection_for_new_app(name, client, &label, interact, resource_type)
                .await?
            {
                ResourceSelection::Existing(r) => r,
                ResourceSelection::New(r) => {
                    match resource_type {
                        ResourceType::Database => {
                            client
                                .create_database(r.clone(), None)
                                .await
                                .context("Could not create database")?;
                        }
                        ResourceType::KeyValueStore => {
                            client
                                .create_key_value_store(&r, None)
                                .await
                                .context("Could not create key value store")?;
                        }
                    }
                    r
                }
                // User canceled terminal interaction
                ResourceSelection::Cancelled => return Ok(None),
            };
        resources_to_link.push((r, label));
    }
    Ok(Some(resources_to_link))
}

// Loops through an updated app's manifest and creates and links any newly referenced resources.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_and_link_resources_for_existing_app(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
    resource_type: ResourceType,
) -> anyhow::Result<Option<()>> {
    for label in labels {
        let resource_label = ResourceLabel {
            app_id,
            label,
            app_name: Some(app_name.to_string()),
        };
        if let ExistingAppDatabaseSelection::NotYetLinked(selection) =
            get_resource_selection_for_existing_app(
                app_name,
                client,
                &resource_label,
                interact,
                resource_type,
            )
            .await?
        {
            match selection {
                // User canceled terminal interaction
                ResourceSelection::Cancelled => return Ok(None),
                ResourceSelection::New(r) => match resource_type {
                    ResourceType::Database => {
                        client
                            .create_database(r.clone(), Some(resource_label))
                            .await?;
                    }
                    ResourceType::KeyValueStore => {
                        client
                            .create_key_value_store(&r, Some(resource_label))
                            .await?;
                    }
                },
                ResourceSelection::Existing(r) => match resource_type {
                    ResourceType::Database => {
                        client
                            .create_database_link(&r, resource_label)
                            .await
                            .with_context(|| {
                                format!(
                                    r#"Could not link {resource_type} "{}" to app "{}""#,
                                    r, app_name,
                                )
                            })?;
                    }
                    ResourceType::KeyValueStore => {
                        client
                            .create_key_value_store_link(&r, resource_label)
                            .await
                            .with_context(|| {
                                format!(
                                    r#"Could not link {resource_type} "{}" to app "{}""#,
                                    r, app_name,
                                )
                            })?;
                    }
                },
            }
        }
    }
    Ok(Some(()))
}

pub(super) async fn link_resources(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: Uuid,
    resource_labels: Vec<(String, String)>,
    resource_type: ResourceType,
) -> anyhow::Result<()> {
    for (resource, label) in resource_labels {
        let resource_label = ResourceLabel {
            label,
            app_id,
            app_name: Some(app_name.to_owned()),
        };
        match resource_type {
            ResourceType::Database => {
                client
                    .create_database_link(&resource, resource_label)
                    .await
                    .with_context(|| {
                        format!(
                            r#"Failed to link {resource_type} "{}" to app "{}""#,
                            resource, app_name
                        )
                    })?;
            }
            ResourceType::KeyValueStore => {
                client
                    .create_key_value_store_link(&resource, resource_label)
                    .await
                    .with_context(|| {
                        format!(
                            r#"Failed to link {resource_type} "{}" to app "{}""#,
                            resource, app_name
                        )
                    })?;
            }
        }
    }
    Ok(())
}
