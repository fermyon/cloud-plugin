/// This module provides functions for printing links in various formats
use anyhow::Result;
use clap::ValueEnum;
use cloud_openapi::models::ResourceLabel;
use comfy_table::presets::ASCII_BORDERS_ONLY_CONDENSED;
use dialoguer::Input;
use serde::Serialize;
use std::collections::BTreeMap;

use super::link::Link;

#[derive(ValueEnum, Clone, Debug)]
pub enum ListFormat {
    Table,
    Json,
}

pub struct ResourceLinks {
    pub name: String,
    pub links: Vec<ResourceLabel>,
}

impl ResourceLinks {
    pub fn new(name: String, links: Vec<ResourceLabel>) -> Self {
        Self { name, links }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResourceGroupBy {
    App,
    Resource(ResourceType),
}

impl std::fmt::Display for ResourceGroupBy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceGroupBy::App => f.write_str("app"),
            ResourceGroupBy::Resource(ResourceType::Database) => f.write_str("database"),
            // TODO consider renaming to "key_value_store"
            ResourceGroupBy::Resource(ResourceType::KeyValueStore) => {
                f.write_str("key_value_store")
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ResourceType {
    Database,
    KeyValueStore,
}

impl std::fmt::Display for ResourceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResourceType::Database => f.write_str("database"),
            ResourceType::KeyValueStore => f.write_str("key value store"),
        }
    }
}

pub fn print_json(
    mut links: Vec<ResourceLinks>,
    app_filter: Option<&str>,
    resource_type: ResourceType,
) -> Result<()> {
    if let Some(app) = app_filter {
        links.retain(|d| {
            d.links
                .iter()
                .any(|l| l.app_name.as_deref().unwrap_or("UNKNOWN") == app)
        });
    }
    let json_vals: Vec<_> = links
        .iter()
        .map(|l| json_list_format(l, resource_type))
        .collect();
    let json_text = serde_json::to_string_pretty(&json_vals)?;
    println!("{}", json_text);
    Ok(())
}

pub fn print_table(
    links: Vec<ResourceLinks>,
    app_filter: Option<&str>,
    group_by: Option<ResourceGroupBy>,
    resource_type: ResourceType,
) -> Result<()> {
    let resources_without_links = links.iter().filter(|db| db.links.is_empty());

    let mut links = links
        .iter()
        .flat_map(|db| {
            db.links.iter().map(|l| Link {
                resource: db.name.clone(),
                resource_label: l.clone(),
            })
        })
        .collect::<Vec<_>>();
    let grouping = group_by.unwrap_or(ResourceGroupBy::App);
    if let Some(name) = app_filter {
        links.retain(|l| l.app_name() == name);
        if links.is_empty() {
            println!("No {} linked to an app named '{}'", resource_type, name);
            return Ok(());
        }
    }
    match grouping {
        ResourceGroupBy::App => print_apps(
            links,
            resources_without_links,
            resource_type,
            app_filter.is_none(),
        ),
        ResourceGroupBy::Resource(_) => {
            print_resources(links, resources_without_links, resource_type)
        }
    }
    Ok(())
}

fn json_list_format(
    resource: &ResourceLinks,
    resource_type: ResourceType,
) -> ResourceLinksJson<'_> {
    let links = resource
        .links
        .iter()
        .map(|l| ResourceLabelJson {
            label: l.label.as_str(),
            app: l.app_name.as_deref().unwrap_or("UNKNOWN"),
        })
        .collect();
    match resource_type {
        ResourceType::Database => ResourceLinksJson::Database(DatabaseLinksJson {
            database: resource.name.as_str(),
            links,
        }),
        ResourceType::KeyValueStore => ResourceLinksJson::KeyValueStore(KeyValueStoreLinksJson {
            key_value_store: resource.name.as_str(),
            links,
        }),
    }
}

#[derive(Serialize)]
#[serde(untagged)]
enum ResourceLinksJson<'a> {
    Database(DatabaseLinksJson<'a>),
    KeyValueStore(KeyValueStoreLinksJson<'a>),
}

#[derive(Serialize)]
struct KeyValueStoreLinksJson<'a> {
    key_value_store: &'a str,
    links: Vec<ResourceLabelJson<'a>>,
}

#[derive(Serialize)]
struct DatabaseLinksJson<'a> {
    database: &'a str,
    links: Vec<ResourceLabelJson<'a>>,
}

/// A ResourceLabel type without app ID for JSON output
#[derive(Serialize)]
struct ResourceLabelJson<'a> {
    label: &'a str,
    app: &'a str,
}

/// Print apps optionally filtering to a specifically supplied app and/or database
fn print_apps<'a>(
    mut links: Vec<Link>,
    resources_without_links: impl Iterator<Item = &'a ResourceLinks>,
    resource_type: ResourceType,
    print_unlinked: bool,
) {
    let resource_descriptor = resource_type.to_string();
    links.sort_by(|l1, l2| l1.app_name().cmp(l2.app_name()));

    let mut table = comfy_table::Table::new();
    table.load_preset(ASCII_BORDERS_ONLY_CONDENSED);
    table.set_header(vec!["App", "Label", &titlecase(&resource_descriptor)]);

    let rows = links.iter().map(|link| {
        [
            link.app_name(),
            link.resource_label.label.as_str(),
            link.resource.as_str(),
        ]
    });
    table.add_rows(rows);
    println!("{table}");

    let mut databases_without_links = resources_without_links.peekable();
    if databases_without_links.peek().is_none() {
        return;
    }
    if print_unlinked {
        let mut table = comfy_table::Table::new();
        println!(
            "{}s not linked to any app",
            capitalize(&resource_descriptor)
        );
        table.set_header(vec![&titlecase(&resource_descriptor)]);
        table.add_rows(databases_without_links.map(|d| [&d.name]));
        println!("{table}");
    }
}

/// Print databases optionally filtering to a specifically supplied app and/or database
fn print_resources<'a>(
    mut links: Vec<Link>,
    resources_without_links: impl Iterator<Item = &'a ResourceLinks>,
    resource_type: ResourceType,
) {
    links.sort_by(|l1, l2| l1.resource.cmp(&l2.resource));

    let mut table = comfy_table::Table::new();
    table.load_preset(ASCII_BORDERS_ONLY_CONDENSED);
    table.set_header(vec![&titlecase(&resource_type.to_string()), "Links"]);
    table.add_rows(resources_without_links.map(|d| [&d.name, "-"]));

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

// Uppercase the first letter of each word in a string
pub fn titlecase(s: &str) -> String {
    s.split_whitespace()
        .map(capitalize)
        .collect::<Vec<_>>()
        .join(" ")
}

// Uppercase the first letter of a string
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

pub fn find_resource_link(store: &ResourceLinks, label: &str) -> Option<Link> {
    store.links.iter().find_map(|r| {
        if r.label == label {
            Some(Link::new(r.clone(), store.name.clone()))
        } else {
            None
        }
    })
}

pub fn prompt_delete_resource(
    name: &str,
    links: &[ResourceLabel],
    resource_type: ResourceType,
) -> std::io::Result<bool> {
    let existing_links = links
        .iter()
        .map(|l| l.app_name.as_deref().unwrap_or("UNKNOWN"))
        .collect::<Vec<&str>>()
        .join(", ");
    let mut prompt = String::new();
    if !existing_links.is_empty() {
        // TODO: use warning color text
        prompt.push_str(&format!("{} \"{name}\" is currently linked to the following apps: {existing_links}.\n\
        It is recommended to use `spin cloud link sqlite` to link another {resource_type} to those apps before deleting.\n", capitalize(&resource_type.to_string())))
    }
    prompt.push_str(&format!(
        "The action is irreversible. Please type \"{name}\" for confirmation"
    ));
    let mut input = Input::<String>::new();
    input.with_prompt(prompt);
    let answer = input.interact_text()?;
    if answer != name {
        println!("Invalid confirmation. Will not delete {resource_type}.");
        Ok(false)
    } else {
        println!("Deleting {resource_type} ...");
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_titlecase() {
        assert_eq!(titlecase("hello world"), "Hello World");
        assert_eq!(titlecase("hello"), "Hello");
        assert_eq!(titlecase("Hello"), "Hello");
        assert_eq!(titlecase("HELLO"), "HELLO");
        assert_eq!(titlecase(""), "");
    }

    #[test]
    fn test_json_list_format() {
        let link = ResourceLinks::new(
            "db1".to_string(),
            vec![ResourceLabel {
                app_id: uuid::Uuid::new_v4(),
                label: "label1".to_string(),
                app_name: Some("app1".to_string()),
            }],
        );
        if let ResourceLinksJson::Database(json) = json_list_format(&link, ResourceType::Database) {
            assert_eq!(json.database, "db1");
            assert_eq!(json.links.len(), 1);
            assert_eq!(json.links[0].label, "label1");
            assert_eq!(json.links[0].app, "app1");
        } else {
            panic!("Expected Database type")
        }
    }
}
