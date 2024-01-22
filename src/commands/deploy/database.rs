// TODO(kate): rename this module to something more generic like `resource`
use anyhow::{anyhow, bail, Context, Result};
use cloud::CloudClientInterface;
use cloud_openapi::models::ResourceLabel;

use crate::commands::links_output::ResourceLinks;
use crate::commands::links_output::ResourceType;
use crate::random_name::RandomNameGenerator;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;

use super::LinkageSpec;

/// A user's selection of a resource to link to a label
pub(super) enum ResourceSelection {
    Existing(String),
    New(String),
    Cancelled,
}

/// Whether a resource has already been linked or not
enum ExistingAppResourceSelection {
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
) -> Result<ExistingAppResourceSelection> {
    let resources = get_resources(client, resource_type).await?;
    if resources
        .iter()
        .any(|d| d.has_link(&resource_label.label, resource_label.app_name.as_deref()))
    {
        return Ok(ExistingAppResourceSelection::AlreadyLinked);
    }
    let selection = interact.prompt_resource_selection(
        name,
        &resource_label.label,
        resources,
        resource_type,
    )?;
    Ok(ExistingAppResourceSelection::NotYetLinked(selection))
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
    kv_labels_to_resource: HashMap<String, String>,
    db_labels_to_resource: HashMap<String, String>,
}

impl Scripted {
    pub(super) fn set_label_action(
        &mut self,
        label: &str,
        resource_name: String,
        resource_type: ResourceType,
    ) -> anyhow::Result<()> {
        let labels_to_resource = match resource_type {
            ResourceType::Database => &mut self.db_labels_to_resource,
            ResourceType::KeyValueStore => &mut self.kv_labels_to_resource,
        };
        match labels_to_resource.entry(label.to_owned()) {
            Entry::Occupied(_) => bail!("Label {label} is linked more than once"),
            Entry::Vacant(e) => e.insert(resource_name),
        };
        Ok(())
    }
}

impl InteractionStrategy for Scripted {
    fn prompt_resource_selection(
        &self,
        _name: &str,
        label: &str,
        resources: Vec<ResourceLinks>,
        resource_type: ResourceType,
    ) -> Result<ResourceSelection> {
        let existing_names: HashSet<&str> = resources
            .iter()
            .map(|resource| resource.name.as_str())
            .collect();
        let requested_resource = self.resource_for(label, resource_type)?;
        if existing_names.contains(requested_resource) {
            Ok(ResourceSelection::Existing(requested_resource.to_owned()))
        } else {
            Ok(ResourceSelection::New(requested_resource.to_owned()))
        }
    }
}

impl Scripted {
    fn resource_for(&self, label: &str, resource_type: ResourceType) -> anyhow::Result<&str> {
        let resource = match resource_type {
            ResourceType::Database => self.db_labels_to_resource.get(label),
            ResourceType::KeyValueStore => self.kv_labels_to_resource.get(label),
        };
        match resource {
            Some(resource_ref) => Ok(resource_ref),
            None => Err(anyhow!("No link specified for label '{label}'")),
        }
    }
}

// Loops through an app's manifest and creates resources.
// Returns a list of linkages that should be resolved
// once the app is created.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_resources_for_new_app(
    client: &impl CloudClientInterface,
    app_name: &str,
    db_labels: HashSet<String>,
    kv_labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
) -> anyhow::Result<Option<Vec<LinkageSpec>>> {
    let mut resources_to_link: Vec<LinkageSpec> = Vec::new();
    let db_label_types = db_labels.into_iter().map(|l| (l, ResourceType::Database));
    let kv_label_types = kv_labels
        .into_iter()
        .map(|l| (l, ResourceType::KeyValueStore));
    let label_types = db_label_types
        .chain(kv_label_types)
        .collect::<Vec<(_, _)>>();
    println!("Creating resources {label_types:?}");
    for (label, resource_type) in label_types {
        let resource = match get_resource_selection_for_new_app(
            app_name,
            client,
            &label,
            interact,
            resource_type,
        )
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
        resources_to_link.push(LinkageSpec::new(label, resource, resource_type));
    }
    Ok(Some(resources_to_link))
}

// Loops through an updated app's manifest and creates and links any newly referenced resources.
// Returns None if the user canceled terminal interaction
pub(super) async fn create_and_link_resources_for_existing_app(
    client: &impl CloudClientInterface,
    app_name: &str,
    app_id: uuid::Uuid,
    db_labels: HashSet<String>,
    kv_labels: HashSet<String>,
    interact: &dyn InteractionStrategy,
) -> anyhow::Result<Option<()>> {
    let db_label_types = db_labels.into_iter().map(|l| (l, ResourceType::Database));
    let kv_label_types = kv_labels
        .into_iter()
        .map(|l| (l, ResourceType::KeyValueStore));
    let label_types = db_label_types
        .chain(kv_label_types)
        .collect::<Vec<(_, _)>>();
    for (label, resource_type) in label_types {
        let resource_label = ResourceLabel {
            app_id,
            label,
            app_name: Some(app_name.to_string()),
        };
        if let ExistingAppResourceSelection::NotYetLinked(selection) =
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
                ResourceSelection::New(resource) => match resource_type {
                    ResourceType::Database => {
                        client
                            .create_database(resource, Some(resource_label))
                            .await?;
                    }
                    ResourceType::KeyValueStore => {
                        client
                            .create_key_value_store(&resource, Some(resource_label))
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
    linkages: Vec<LinkageSpec>,
) -> anyhow::Result<()> {
    for link in linkages {
        let resource_label = ResourceLabel {
            label: link.label,
            app_id,
            app_name: Some(app_name.to_owned()),
        };
        match link.resource_type {
            ResourceType::Database => {
                client
                    .create_database_link(&link.resource_name, resource_label)
                    .await
                    .with_context(|| {
                        format!(
                            r#"Failed to link {} "{}" to app "{}""#,
                            link.resource_type, link.resource_name, app_name
                        )
                    })?;
            }
            ResourceType::KeyValueStore => {
                client
                    .create_key_value_store_link(&link.resource_name, resource_label)
                    .await
                    .with_context(|| {
                        format!(
                            r#"Failed to link {} "{}" to app "{}""#,
                            link.resource_type, link.resource_name, app_name
                        )
                    })?;
            }
        }
    }
    Ok(())
}
