use crate::commands::links_output::{capitalize, find_resource_link, ResourceLinks, ResourceType};
use crate::commands::{client_and_app_id, CommonArgs};
use anyhow::{Context, Result};
use clap::Parser;
use cloud::CloudClientInterface;
use cloud_openapi::models::ResourceLabel;
use uuid::Uuid;

/// Manage how apps and resources are linked together
#[derive(Parser, Debug)]
pub enum LinkCommand {
    /// Link an app to a SQLite database
    Sqlite(SqliteLinkCommand),
    #[clap(alias = "kv")]
    KeyValueStore(KeyValueStoreLinkCommand),
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

#[derive(Parser, Debug)]
pub struct KeyValueStoreLinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// The name by which the application will refer to the key value store
    label: String,
    #[clap(short = 'a', long = "app")]
    /// The app that will be using the key value store
    app: String,
    /// The key value store that the app will refer to by the label
    #[clap(short = 's', long = "store")]
    store: String,
}

impl LinkCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Sqlite(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                cmd.link(client, app_id).await
            }
            Self::KeyValueStore(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                cmd.link(client, app_id).await
            }
        }
    }
}

impl SqliteLinkCommand {
    async fn link(self, client: impl CloudClientInterface, app_id: Uuid) -> Result<()> {
        let stores = client
            .get_databases(None)
            .await
            .context("could not fetch key value stores")?;
        let resources = stores
            .into_iter()
            .map(|s| ResourceLinks::new(s.name, s.links))
            .collect::<Vec<_>>();
        link(
            client,
            &self.database,
            &self.app,
            &self.label,
            app_id,
            resources,
            ResourceType::Database,
        )
        .await
    }
}

impl KeyValueStoreLinkCommand {
    async fn link(self, client: impl CloudClientInterface, app_id: Uuid) -> Result<()> {
        let stores = client
            .get_key_value_stores(None)
            .await
            .context("could not fetch key value stores")?;
        let resources = stores
            .into_iter()
            .map(|s| ResourceLinks::new(s.name, s.links))
            .collect::<Vec<_>>();
        link(
            client,
            &self.store,
            &self.app,
            &self.label,
            app_id,
            resources,
            ResourceType::KeyValueStore,
        )
        .await
    }
}

async fn link(
    client: impl CloudClientInterface,
    resource_name: &str,
    app: &str,
    label: &str,
    app_id: Uuid,
    resources: Vec<ResourceLinks>,
    resource_type: ResourceType,
) -> Result<()> {
    let exists = resources.iter().any(|s| s.name == resource_name);
    if !exists {
        anyhow::bail!(
            r#"{} "{}" does not exist"#,
            capitalize(&resource_type.to_string()),
            resource_name
        );
    }
    let stores_for_app = resources
        .into_iter()
        .filter(|s| s.links.iter().any(|l| l.app_id == app_id))
        .collect::<Vec<_>>();
    let (this_store, other_stores): (Vec<&ResourceLinks>, Vec<&ResourceLinks>) =
        stores_for_app.iter().partition(|d| d.name == resource_name);
    let existing_link_for_store = this_store.iter().find_map(|s| find_resource_link(s, label));
    let existing_link_for_other_store = other_stores
        .iter()
        .find_map(|s| find_resource_link(s, label));

    let success_msg = format!(
        r#"{} "{}" is now linked to app "{}" with the label "{}""#,
        capitalize(&resource_type.to_string()),
        resource_name,
        app,
        label
    );
    match (existing_link_for_store, existing_link_for_other_store) {
        (Some(link), _) => {
            anyhow::bail!(
                r#"{} "{}" is already linked to app "{}" with the label "{}""#,
                capitalize(&resource_type.to_string()),
                link.resource,
                link.app_name(),
                link.resource_label.label,
            );
        }
        (_, Some(link)) => {
            let prompt = format!(
                r#"App "{}"'s "{}" label is currently linked to "{}". Change to link to {} "{}" instead?"#,
                link.app_name(),
                link.resource_label.label,
                link.resource,
                resource_type,
                resource_name,
            );
            if dialoguer::Confirm::new()
                .with_prompt(prompt)
                .default(false)
                .interact_opt()?
                .unwrap_or_default()
            {
                match resource_type {
                    ResourceType::Database => {
                        client
                            .remove_database_link(&link.resource, link.resource_label)
                            .await?
                    }
                    ResourceType::KeyValueStore => {
                        client
                            .remove_key_value_store_link(&link.resource, link.resource_label)
                            .await?
                    }
                }

                let resource_label = ResourceLabel {
                    app_id,
                    label: label.to_string(),
                    app_name: None,
                };

                match resource_type {
                    ResourceType::Database => {
                        client
                            .create_database_link(resource_name, resource_label)
                            .await?
                    }
                    ResourceType::KeyValueStore => {
                        client
                            .create_key_value_store_link(resource_name, resource_label)
                            .await?
                    }
                }
                println!("{success_msg}");
            } else {
                println!("The link has not been updated");
            }
        }
        (None, None) => {
            let resource_label = ResourceLabel {
                app_id,
                label: label.to_string(),
                app_name: None,
            };
            match resource_type {
                ResourceType::Database => {
                    client
                        .create_database_link(resource_name, resource_label)
                        .await?
                }
                ResourceType::KeyValueStore => {
                    client
                        .create_key_value_store_link(resource_name, resource_label)
                        .await?
                }
            }
            println!("{success_msg}");
        }
    }
    Ok(())
}

/// Manage unlinking apps and resources
#[derive(Parser, Debug)]
pub enum UnlinkCommand {
    /// Unlink an app from a SQLite database
    Sqlite(SqliteUnlinkCommand),
    /// Unlink an app from a key value store
    #[clap(alias = "kv")]
    KeyValueStore(KeyValueStoreUnlinkCommand),
}

impl UnlinkCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Sqlite(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                cmd.unlink(client, app_id).await
            }
            Self::KeyValueStore(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                cmd.unlink(client, app_id).await
            }
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
    async fn unlink(self, client: impl CloudClientInterface, app_id: Uuid) -> Result<()> {
        let databases = client
            .get_databases(Some(app_id))
            .await
            .context("could not fetch databases")?;
        let resources = databases
            .into_iter()
            .map(|s| ResourceLinks::new(s.name, s.links))
            .collect::<Vec<_>>();
        unlink(
            client,
            &self.app,
            &self.label,
            resources,
            ResourceType::Database,
        )
        .await
    }
}

#[derive(Parser, Debug)]
pub struct KeyValueStoreUnlinkCommand {
    #[clap(flatten)]
    common: CommonArgs,
    /// The name by which the application refers to the key value store
    label: String,
    #[clap(short = 'a', long = "app")]
    /// The app that will be using the key value store
    app: String,
}

impl KeyValueStoreUnlinkCommand {
    async fn unlink(self, client: impl CloudClientInterface, app_id: Uuid) -> Result<()> {
        let stores = client
            .get_key_value_stores(Some(app_id))
            .await
            .context("could not fetch key value stores")?;
        let resources = stores
            .into_iter()
            .map(|s| ResourceLinks::new(s.name, s.links))
            .collect::<Vec<_>>();
        unlink(
            client,
            &self.app,
            &self.label,
            resources,
            ResourceType::KeyValueStore,
        )
        .await
    }
}

pub async fn unlink(
    client: impl CloudClientInterface,
    app: &str,
    label: &str,
    resources: Vec<ResourceLinks>,
    resource_type: ResourceType,
) -> Result<()> {
    let (resource_name, resource_label) = resources
        .into_iter()
        .find_map(|d| {
            d.links
                .into_iter()
                .find(|l| {
                    matches!(&l.app_name, Some(app_name) if app_name == app) && l.label == label
                })
                .map(|l| (d.name, l))
        })
        .with_context(|| format!("no database was linked to app '{app}' with label '{label}'"))?;
    match resource_type {
        ResourceType::Database => {
            client
                .remove_database_link(&resource_name, resource_label)
                .await?
        }
        ResourceType::KeyValueStore => {
            client
                .remove_key_value_store_link(&resource_name, resource_label)
                .await?
        }
    }
    println!(
        "{} '{resource_name}' no longer linked to app {app}",
        capitalize(&resource_type.to_string())
    );
    Ok(())
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
    use cloud_openapi::models::{Database, KeyValueStoreItem};
    #[tokio::test]
    async fn test_sqlite_link_error_database_does_not_exist() -> Result<()> {
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

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().return_once(move |_| Ok(dbs));

        let result = command.link(mock, app_id).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"Database "does-not-exist" does not exist"#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_sqlite_link_succeeds_when_database_exists() -> Result<()> {
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
        let expected_resource_label = ResourceLabel {
            app_id,
            label: command.label.clone(),
            app_name: None,
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().return_once(move |_| Ok(dbs));
        mock.expect_create_database_link()
            .withf(move |db, rl| db == "db1" && rl == &expected_resource_label)
            .returning(|_, _| Ok(()));

        command.link(mock, app_id).await
    }

    #[tokio::test]
    async fn test_sqlite_link_errors_when_link_already_exists() -> Result<()> {
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

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_databases().return_once(move |_| Ok(dbs));
        let result = command.link(mock, app_id).await;

        assert_eq!(
            result.unwrap_err().to_string(),
            r#"Database "db1" is already linked to app "app" with the label "label""#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_key_value_store_link_error_store_does_not_exist() -> Result<()> {
        let command = KeyValueStoreLinkCommand {
            app: "app".to_string(),
            store: "does-not-exist".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            KeyValueStoreItem::new("kv1".to_string(), vec![]),
            KeyValueStoreItem::new("kv2".to_string(), vec![]),
        ];

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_key_value_stores()
            .return_once(move |_| Ok(dbs));

        let result = command.link(mock, app_id).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            r#"Key value store "does-not-exist" does not exist"#
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_key_value_store_link_succeeds_when_store_exists() -> Result<()> {
        let command = KeyValueStoreLinkCommand {
            app: "app".to_string(),
            store: "kv1".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            KeyValueStoreItem::new("kv1".to_string(), vec![]),
            KeyValueStoreItem::new("kv2".to_string(), vec![]),
        ];
        let expected_resource_label = ResourceLabel {
            app_id,
            label: command.label.clone(),
            app_name: None,
        };

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_key_value_stores()
            .return_once(move |_| Ok(dbs));
        mock.expect_create_key_value_store_link()
            .withf(move |db, rl| db == "kv1" && rl == &expected_resource_label)
            .returning(|_, _| Ok(()));

        command.link(mock, app_id).await
    }

    #[tokio::test]
    async fn test_key_value_store_unlink_error_store_does_not_exist() -> Result<()> {
        let command = KeyValueStoreUnlinkCommand {
            app: "app".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            KeyValueStoreItem::new(
                "kv1".to_string(),
                vec![ResourceLabel {
                    app_id,
                    label: "other".to_string(),
                    app_name: Some("bar".to_string()),
                }],
            ),
            KeyValueStoreItem::new("kv2".to_string(), vec![]),
        ];

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_key_value_stores()
            .return_once(move |_| Ok(dbs));

        let result = command.unlink(mock, app_id).await;
        assert_eq!(
            result.unwrap_err().to_string(),
            "no database was linked to app 'app' with label 'label'"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_key_value_store_unlink_succeeds_when_link_exists() -> Result<()> {
        let command = KeyValueStoreUnlinkCommand {
            app: "app".to_string(),
            label: "label".to_string(),
            common: Default::default(),
        };
        let app_id = Uuid::new_v4();
        let dbs = vec![
            KeyValueStoreItem::new(
                "kv1".to_string(),
                vec![ResourceLabel {
                    app_id,
                    label: command.label.clone(),
                    app_name: Some("app".to_string()),
                }],
            ),
            KeyValueStoreItem::new("kv2".to_string(), vec![]),
        ];

        let mut mock = MockCloudClientInterface::new();
        mock.expect_get_key_value_stores()
            .return_once(move |_| Ok(dbs));
        mock.expect_remove_key_value_store_link()
            .returning(|_, _| Ok(()));

        command.unlink(mock, app_id).await
    }

    // TODO: add test test_sqlite_link_errors_when_link_exists_with_different_database()
    // once there is a flag to avoid prompts
}
