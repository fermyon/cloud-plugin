use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use cloud::{
    client::{Client as CloudClient, ConnectionConfig},
    CloudClientExt, CloudClientInterface,
};
use oci_distribution::{token_cache, Reference, RegistryOperation};
use spin_common::arg_parser::parse_kv;
use spin_http::{app_info::AppInfo, config::HttpTriggerRouteConfig, routes::Router};
use spin_locked_app::locked;
use spin_oci::ComposeMode;
use tokio::fs;
use tracing::instrument;

use std::{
    collections::HashSet,
    io::{self, Write},
    path::{Path, PathBuf},
    str::FromStr,
};
use url::Url;

use crate::{
    commands::{
        links_output::ResourceType,
        variables::{get_variables, set_variables},
        DEFAULT_CLOUD_URL,
    },
    spin,
};

use crate::{
    commands::login::{LoginCommand, LoginConnection},
    opts::*,
};

mod resource;

const DEVELOPER_CLOUD_FAQ: &str = "https://developer.fermyon.com/cloud/faq";
const SPIN_DEFAULT_KV_STORE: &str = "default";

/// The amount of time a token must have remaining before expiry for us to be
/// confident it will last long enough to complete a deploy operation. That is,
/// if a token is closer than this to expiration when we start a deploy
/// operation, we should refresh it pre-emptively so that it's unlikely to expire
/// while the operation is in progress.
const TOKEN_MUST_HAVE_REMAINING: chrono::TimeDelta = chrono::TimeDelta::minutes(5);

// When we come to list features here, you can find consts for them in `spin_locked_app`
// e.g. spin_locked_app::locked::SERVICE_CHAINING_KEY.
const CLOUD_SUPPORTED_FEATURES: &[&str] = &[];

/// Package and upload an application to the Fermyon Cloud.
#[derive(Parser, Debug)]
#[clap(about = "Package and upload an application to the Fermyon Cloud")]
pub struct DeployCommand {
    /// The application to deploy. This may be a manifest (spin.toml) file, a
    /// directory containing a spin.toml file, or a remote registry reference.
    /// If omitted, it defaults to "spin.toml".
    #[clap(
        name = APPLICATION_OPT,
        short = 'f',
        long = "from",
        group = "source",
    )]
    pub app_source: Option<String>,

    /// The application to deploy. This is the same as `--from` but forces the
    /// application to be interpreted as a file or directory path.
    #[clap(
        hide = true,
        name = APP_MANIFEST_FILE_OPT,
        long = "from-file",
        alias = "file",
        group = "source",
    )]
    pub file_source: Option<PathBuf>,

    /// The application to deploy. This is the same as `--from` but forces the
    /// application to be interpreted as an OCI registry reference.
    #[clap(
        hide = true,
        name = FROM_REGISTRY_OPT,
        long = "from-registry",
        group = "source",
    )]
    pub registry_source: Option<String>,

    /// For local apps, specifies to perform `spin build` before deploying the application.
    ///
    /// This is ignored on remote applications, as they are already built.
    #[clap(long, takes_value = false, env = "SPIN_ALWAYS_BUILD")]
    pub build: bool,

    /// How long in seconds to wait for a deployed HTTP application to become
    /// ready. The default is 60 seconds. Set it to 0 to skip waiting
    /// for readiness.
    #[clap(long = "readiness-timeout", default_value = "60")]
    pub readiness_timeout_secs: u16,

    /// Deploy to the Fermyon instance saved under the specified name.
    /// If omitted, Spin deploys to the default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV,
        hidden = true
    )]
    pub deployment_env_id: Option<String>,

    /// Set a key/value pair (key=value) in the deployed application's
    /// default store. Any existing value will be overwritten.
    /// Can be used multiple times.
    #[clap(long = "key-value", parse(try_from_str = parse_kv))]
    pub key_values: Vec<(String, String)>,

    /// Set a variable (variable=value) in the deployed application.
    /// Any existing value will be overwritten.
    /// Can be used multiple times.
    #[clap(long = "variable", parse(try_from_str = parse_kv))]
    pub variables: Vec<(String, String)>,

    /// Specifies how application labels (such as SQLite databases) should
    /// be linked if they are not already linked. This is intended for
    /// non-interactive environments such as release pipelines; therefore,
    /// if any links are specified, all links must be specified.
    ///
    /// Links must be of the form 'sqlite:label=database' or
    /// 'kv:label=store'. Databases or key value stores that do not exist
    /// will be created.
    #[clap(long = "link")]
    pub links: Vec<String>,
}

impl DeployCommand {
    pub async fn run(self) -> Result<()> {
        if self.build {
            self.run_spin_build().await?;
        }

        let login_connection = login_connection(self.deployment_env_id.as_deref()).await?;

        self.deploy_cloud(login_connection)
            .await
            .map_err(|e| anyhow!("{:?}\n\nLearn more at {}", e, DEVELOPER_CLOUD_FAQ))
    }

    fn resolve_app_source(&self) -> AppSource {
        match (&self.app_source, &self.file_source, &self.registry_source) {
            (None, None, None) => self.default_manifest_or_none(),
            (Some(source), None, None) => Self::infer_source(source),
            (None, Some(file), None) => Self::infer_file_source(file.to_owned()),
            (None, None, Some(reference)) => AppSource::OciRegistry(reference.to_owned()),
            _ => AppSource::unresolvable("More than one application source was specified"),
        }
    }

    fn default_manifest_or_none(&self) -> AppSource {
        let default_manifest = PathBuf::from(DEFAULT_MANIFEST_FILE);
        if default_manifest.exists() {
            AppSource::File(default_manifest)
        } else {
            AppSource::None
        }
    }

    fn infer_source(source: &str) -> AppSource {
        let path = PathBuf::from(source);
        if path.exists() {
            Self::infer_file_source(path)
        } else if spin_oci::is_probably_oci_reference(source) {
            AppSource::OciRegistry(source.to_owned())
        } else {
            AppSource::Unresolvable(format!("File or directory '{source}' not found. If you meant to load from a registry, use the `--from-registry` option."))
        }
    }

    fn infer_file_source(path: impl Into<PathBuf>) -> AppSource {
        match spin_common::paths::resolve_manifest_file_path(path.into()) {
            Ok(file) => AppSource::File(file),
            Err(e) => AppSource::Unresolvable(e.to_string()),
        }
    }

    async fn deploy_cloud(self, login_connection: LoginConnection) -> Result<()> {
        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };

        let client = CloudClient::new(connection_config.clone());
        let interact = self.interaction_strategy()?;

        let dir = tempfile::tempdir()?;

        let application = self.load_cloud_app(dir.path()).await?;

        validate_cloud_app(&application)?;
        self.validate_deployment_environment(&application, &client)
            .await?;

        let digest = self
            .push_oci(application.clone(), connection_config.clone())
            .await?;

        let name = sanitize_app_name(application.name()?);
        let storage_id = format!("oci://{}", name);
        let version = sanitize_app_version(application.version()?);

        let kv_labels = application.key_value_stores();
        if !kv_labels.contains(SPIN_DEFAULT_KV_STORE) && !self.key_values.is_empty() {
            bail!("The `key_values` flag can only be used to set key/value pairs in the default key/value store. The application does not reference a key/value store with the label 'default'");
        }
        let db_labels = application.sqlite_databases();

        println!("Deploying...");

        // Create or update app
        let app_id = match client.get_app_id(&name).await? {
            Some(app_id) => {
                resource::create_and_link_resources_for_existing_app(
                    &client,
                    &name,
                    app_id,
                    db_labels,
                    kv_labels,
                    interact.as_ref(),
                )
                .await?;
                client
                    .add_revision(storage_id.clone(), version.clone())
                    .await?;
                // We have already checked that default kv store exists
                for kv in self.key_values {
                    client
                        .add_key_value_pair(
                            Some(app_id),
                            SPIN_DEFAULT_KV_STORE.to_string(),
                            kv.0,
                            kv.1,
                        )
                        .await
                        .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                app_id
            }
            None => {
                let resources_to_link = match resource::create_resources_for_new_app(
                    &client,
                    &name,
                    db_labels,
                    kv_labels,
                    interact.as_ref(),
                )
                .await?
                {
                    Some(dbs) => dbs,
                    // TODO: Clean up created databases and kv stores
                    None => return Ok(()), // User canceled terminal interaction
                };
                let app_id = client
                    .add_app(&name, &storage_id)
                    .await
                    .context("Unable to create app")?;

                // Now that the app has been created, we can link resources to it.
                resource::link_resources(&client, &name, app_id, resources_to_link).await?;
                client
                    .add_revision(storage_id.clone(), version.clone())
                    .await
                    .context(format!("Unable to upload {}", version.clone()))?;

                // Have already checked that default kv store exists
                for kv in self.key_values {
                    client
                        .add_key_value_pair(
                            Some(app_id),
                            SPIN_DEFAULT_KV_STORE.to_string(),
                            kv.0,
                            kv.1,
                        )
                        .await
                        .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                app_id
            }
        };

        let app = client
            .get_app(app_id.to_string())
            .await
            .context("Problem getting app by id")?;

        let app_base_url = build_app_base_url(&app.subdomain, &login_connection.url)?;
        let (http_base, http_router, _) = application.http_routes()?;
        if http_router.routes().next().is_some() {
            wait_for_ready(
                &app_base_url,
                &digest.unwrap_or_default(),
                self.readiness_timeout_secs,
                Destination::Cloud(connection_config.clone().url),
            )
            .await;
            let base = http_base.unwrap_or("/");
            print_available_routes(&application, &name, &app_base_url, base, &http_router);
        } else {
            println!("Application is running at {}", app.subdomain);
        }

        Ok(())
    }

    fn interaction_strategy(&self) -> anyhow::Result<Box<dyn resource::InteractionStrategy>> {
        if self.links.is_empty() {
            return Ok(Box::new(resource::Interactive));
        }

        let script = parse_linkage_specs(&self.links)?;
        Ok(Box::new(script))
    }

    async fn load_cloud_app(&self, working_dir: &Path) -> Result<DeployableApp, anyhow::Error> {
        let app_source = self.resolve_app_source();

        let locked_app = match &app_source {
            AppSource::File(app_file) => {
                spin_loader::from_file(
                    &app_file,
                    spin_loader::FilesMountStrategy::Copy(working_dir.to_owned()),
                    None,
                )
                .await?
            }
            AppSource::OciRegistry(reference) => {
                let mut oci_client = spin_oci::Client::new(false, None)
                    .await
                    .context("cannot create registry client")?;

                spin_oci::OciLoader::new(working_dir)
                    .load_app(&mut oci_client, reference)
                    .await?
            }
            AppSource::None => {
                anyhow::bail!("Default file '{DEFAULT_MANIFEST_FILE}' not found.");
            }
            AppSource::Unresolvable(err) => {
                anyhow::bail!("{err}");
            }
        };

        let unsupported_triggers = locked_app
            .triggers
            .iter()
            .filter(|t| t.trigger_type != "http")
            .map(|t| format!("'{}'", t.trigger_type))
            .collect::<Vec<_>>();
        if !unsupported_triggers.is_empty() {
            bail!(
                "Non-HTTP triggers are not supported - app uses {}",
                unsupported_triggers.join(", ")
            );
        }

        if let Err(unsupported) = locked_app.ensure_needs_only(CLOUD_SUPPORTED_FEATURES) {
            bail!("This app requires features that are not yet available on Fermyon Cloud: {unsupported}");
        }

        let locked_app = ensure_http_base_set(locked_app);
        let locked_app = ensure_plugin_version_set(locked_app);

        Ok(DeployableApp(locked_app))
    }

    async fn validate_deployment_environment(
        &self,
        app: &DeployableApp,
        client: &CloudClient,
    ) -> Result<()> {
        let required_variables = app
            .0
            .variables
            .iter()
            .filter(|(_, v)| v.default.is_none())
            .map(|(k, _)| k)
            .collect::<HashSet<_>>();
        if !required_variables.is_empty() {
            self.ensure_variables_present(&required_variables, client, app.name()?)
                .await?;
        }
        Ok(())
    }

    async fn ensure_variables_present(
        &self,
        required_variables: &HashSet<&String>,
        client: &CloudClient,
        name: &str,
    ) -> Result<()> {
        // Are all required variables satisifed by variables passed in this command?
        let provided_variables = self.variables.iter().map(|(k, _)| k).collect();
        let unprovided_variables = required_variables
            .difference(&provided_variables)
            .copied()
            .collect::<HashSet<_>>();
        if unprovided_variables.is_empty() {
            return Ok(());
        }

        // Are all remaining required variables satisfied by variables already in the cloud?
        let extant_variables = match client.get_app_id(name).await {
            Ok(Some(app_id)) => match get_variables(client, app_id).await {
                Ok(variables) => variables,
                Err(_) => {
                    // Don't block deployment for being unable to check the variables.
                    eprintln!("Unable to confirm variables {unprovided_variables:?} are defined. Check your app after deployment.");
                    return Ok(());
                }
            },
            Ok(None) => vec![],
            Err(_) => {
                // Don't block deployment for being unable to check the variables.
                eprintln!("Unable to confirm variables {unprovided_variables:?} are defined. Check your app after deployment.");
                return Ok(());
            }
        };
        let extant_variables = extant_variables.iter().map(|v| &v.key).collect();
        let unprovided_variables = unprovided_variables
            .difference(&extant_variables)
            .map(|v| v.as_str())
            .collect::<Vec<_>>();
        if unprovided_variables.is_empty() {
            return Ok(());
        }

        let list_text = unprovided_variables.join(", ");
        Err(anyhow!("The application requires values for the following variable(s) which have not been set: {list_text}. Use the --variable flag to provide values."))
    }

    async fn push_oci(
        &self,
        application: DeployableApp,
        connection_config: ConnectionConfig,
    ) -> Result<Option<String>> {
        let mut client = spin_oci::Client::new(connection_config.insecure, None).await?;

        let cloud_url =
            Url::parse(connection_config.url.as_str()).context("Unable to parse cloud URL")?;
        let cloud_host = cloud_url
            .host_str()
            .context("Unable to derive host from cloud URL")?;
        let cloud_registry_host = format!("registry.{cloud_host}");

        let reference = format!(
            "{}/{}:{}",
            cloud_registry_host,
            &sanitize_app_name(application.name()?),
            &sanitize_app_version(application.version()?)
        );

        let oci_ref = Reference::try_from(reference.as_ref())
            .context(format!("Could not parse reference '{reference}'"))?;

        client
            .insert_token(
                &oci_ref,
                RegistryOperation::Push,
                token_cache::RegistryTokenType::Bearer(token_cache::RegistryToken::Token {
                    token: connection_config.token,
                }),
            )
            .await;

        println!(
            "Uploading {} version {} to Fermyon Cloud...",
            &oci_ref.repository(),
            &oci_ref.tag().unwrap_or(application.version()?)
        );
        // Leave apps uncomposed to enable the Cloud host to deduplicate components.
        let compose_mode = ComposeMode::Skip;
        let digest = client
            .push_locked(
                application.0,
                reference,
                None,
                spin_oci::client::InferPredefinedAnnotations::None,
                compose_mode,
            )
            .await?;

        Ok(digest)
    }

    async fn run_spin_build(&self) -> Result<()> {
        self.resolve_app_source().build().await
    }
}

// Spin now allows HTTP apps to omit the base path, but Cloud
// doesn't yet like this. This works around that by defaulting
// base if not set. (We don't check trigger type because by the
// time this is called we know it's HTTP.)
fn ensure_http_base_set(mut locked_app: locked::LockedApp) -> locked::LockedApp {
    if let Some(trigger) = locked_app
        .metadata
        .entry("trigger")
        .or_insert_with(|| serde_json::Value::Object(Default::default()))
        .as_object_mut()
    {
        trigger.entry("base").or_insert_with(|| "/".into());
    }

    locked_app
}

// Insert cloud plugin version into locked app metadata
fn ensure_plugin_version_set(mut locked_app: locked::LockedApp) -> locked::LockedApp {
    locked_app
        .metadata
        .insert("cloud_plugin_version".to_owned(), crate::VERSION.into());
    locked_app
}

#[derive(Debug, PartialEq, Eq)]
enum AppSource {
    None,
    File(PathBuf),
    OciRegistry(String),
    Unresolvable(String),
}

impl AppSource {
    fn unresolvable(message: impl Into<String>) -> Self {
        Self::Unresolvable(message.into())
    }

    async fn build(&self) -> anyhow::Result<()> {
        match self {
            Self::File(manifest_path) => {
                let spin_bin = spin::bin_path()?;

                let result = tokio::process::Command::new(spin_bin)
                    .args(["build", "-f"])
                    .arg(manifest_path)
                    .status()
                    .await
                    .context("Failed to execute `spin build` command")?;

                if result.success() {
                    Ok(())
                } else {
                    Err(anyhow!("Build failed: deployment cancelled"))
                }
            }
            _ => Ok(()),
        }
    }
}

// SAFE_APP_NAME regex to only allow letters/numbers/underscores/dashes
lazy_static::lazy_static! {
    static ref SAFE_APP_NAME: regex::Regex = regex::Regex::new("^[-_\\p{L}\\p{N}]+$").expect("Invalid name regex");
}

// TODO: logic here inherited from bindle restrictions; it would be friendlier to users
// to be less stringent and do the necessary sanitization on the backend, rather than
// presenting this error at deploy time.
fn check_safe_app_name(name: &str) -> Result<()> {
    if SAFE_APP_NAME.is_match(name) {
        Ok(())
    } else {
        Err(anyhow!("App name '{}' contains characters that are not allowed. It may contain only letters, numbers, dashes and underscores", name))
    }
}

// Sanitize app name to conform to Docker repo name conventions
// From https://docs.docker.com/engine/reference/commandline/tag/#extended-description:
// The path consists of slash-separated components. Each component may contain lowercase letters, digits and separators.
// A separator is defined as a period, one or two underscores, or one or more hyphens. A component may not start or end with a separator.
fn sanitize_app_name(name: &str) -> String {
    let trim_chars = ['.', '_', '-'];
    name.to_ascii_lowercase()
        .replace(' ', "")
        .trim_start_matches(trim_chars)
        .trim_end_matches(trim_chars)
        .to_string()
}

// Sanitize app version to conform to Docker tag conventions
// From https://docs.docker.com/engine/reference/commandline/tag
// A tag name must be valid ASCII and may contain lowercase and uppercase letters, digits, underscores, periods and hyphens.
// A tag name may not start with a period or a hyphen and may contain a maximum of 128 characters.
fn sanitize_app_version(tag: &str) -> String {
    let mut sanitized = tag.trim().trim_start_matches(['.', '-']);

    if sanitized.len() > 128 {
        (sanitized, _) = sanitized.split_at(128);
    }
    sanitized.replace(' ', "")
}

fn validate_cloud_app(app: &DeployableApp) -> Result<()> {
    check_safe_app_name(app.name()?)?;
    ensure!(!app.components().is_empty(), "No components in spin.toml!");
    check_no_duplicate_routes(app)?;
    Ok(())
}

fn check_no_duplicate_routes(app: &DeployableApp) -> Result<()> {
    let (_, _, duplicates) = app.http_routes()?;
    if duplicates.is_empty() {
        Ok(())
    } else {
        let messages: Vec<_> = duplicates
            .iter()
            .map(|dr| {
                format!(
                    "- Route '{}' appears in components '{}' and '{}'",
                    dr.route(),
                    dr.effective_id,
                    dr.replaced_id
                )
            })
            .collect();
        let message = format!("This application contains duplicate routes, which are not allowed in Fermyon Cloud.\n{}", messages.join("\n"));
        bail!(message)
    }
}

#[derive(Clone)]
struct DeployableApp(locked::LockedApp);

struct DeployableComponent(locked::LockedComponent);

impl DeployableApp {
    fn name(&self) -> anyhow::Result<&str> {
        self.0
            .metadata
            .get("name")
            .ok_or(anyhow!("Application has no name"))?
            .as_str()
            .ok_or(anyhow!("Application name is not a string"))
    }

    fn version(&self) -> anyhow::Result<&str> {
        self.0
            .metadata
            .get("version")
            .ok_or(anyhow!("Application has no version"))?
            .as_str()
            .ok_or(anyhow!("Application version is not a string"))
    }

    fn components(&self) -> Vec<DeployableComponent> {
        self.0
            .components
            .iter()
            .map(|c| DeployableComponent(c.clone()))
            .collect()
    }

    fn sqlite_databases(&self) -> HashSet<String> {
        self.components()
            .iter()
            .flat_map(|c| c.sqlite_databases())
            .collect()
    }

    fn key_value_stores(&self) -> HashSet<String> {
        self.components()
            .iter()
            .flat_map(|c| c.key_value_stores())
            .collect()
    }

    fn http_routes(
        &self,
    ) -> anyhow::Result<(Option<&str>, Router, Vec<spin_http::routes::DuplicateRoute>)> {
        let base = self
            .0
            .metadata
            .get("trigger")
            .and_then(|v| v.get("base"))
            .and_then(|v| v.as_str());
        let routes = self
            .0
            .triggers
            .iter()
            .filter_map(|t| self.http_route(t))
            .collect::<Vec<_>>();
        let routes = routes.iter().map(|(id, route)| (id.as_str(), route));
        let (router, duplicates) = Router::build(base.unwrap_or("/"), routes)?;

        Ok((base, router, duplicates))
    }

    fn http_route(
        &self,
        trigger: &locked::LockedTrigger,
    ) -> Option<(String, HttpTriggerRouteConfig)> {
        if &trigger.trigger_type != "http" {
            return None;
        }

        let id = trigger
            .trigger_config
            .get("component")
            .and_then(|v| v.as_str())?;

        let route = trigger
            .trigger_config
            .get("route")
            .and_then(|v| serde_json::from_value(v.clone()).ok())?;
        Some((id.to_owned(), route))
    }

    fn component_description(&self, id: &str) -> Option<&str> {
        self.0
            .components
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.metadata.get("description").and_then(|v| v.as_str()))
    }
}

impl DeployableComponent {
    fn key_value_stores(&self) -> Vec<String> {
        self.metadata_vec_string("key_value_stores")
    }

    fn sqlite_databases(&self) -> Vec<String> {
        self.metadata_vec_string("databases")
    }

    fn metadata_vec_string(&self, key: &str) -> Vec<String> {
        let Some(raw) = self.0.metadata.get(key) else {
            return vec![];
        };
        let Some(arr) = raw.as_array() else {
            return vec![];
        };
        arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_owned())
            .collect()
    }
}

fn build_app_base_url(app_domain: &str, cloud_url: &Url) -> Result<Url> {
    // HACK: We assume that the scheme (https vs http) of apps will match that of Cloud...
    let scheme = cloud_url.scheme();
    Url::parse(&format!("{scheme}://{app_domain}/")).with_context(|| {
        format!("Could not construct app base URL for {app_domain:?} (Cloud URL: {cloud_url:?})",)
    })
}

const READINESS_POLL_INTERVAL_SECS: u64 = 2;

enum Destination {
    Cloud(String),
}

async fn wait_for_ready(
    app_base_url: &Url,
    app_version: &str,
    readiness_timeout_secs: u16,
    destination: Destination,
) {
    if readiness_timeout_secs == 0 {
        return;
    }

    let app_info_url = app_base_url
        .join(spin_http::WELL_KNOWN_PREFIX.trim_start_matches('/'))
        .unwrap()
        .join("info")
        .unwrap()
        .to_string();

    let start = std::time::Instant::now();
    let readiness_timeout = std::time::Duration::from_secs(u64::from(readiness_timeout_secs));
    let poll_interval = tokio::time::Duration::from_secs(READINESS_POLL_INTERVAL_SECS);

    print!("Waiting for application to become ready");
    let _ = std::io::stdout().flush();
    loop {
        match is_ready(&app_info_url, app_version).await {
            Err(err) => {
                println!("... readiness check failed: {err:?}");
                return;
            }
            Ok(true) => {
                println!("... ready");
                return;
            }
            Ok(false) => {}
        }

        print!(".");
        let _ = std::io::stdout().flush();

        if start.elapsed() >= readiness_timeout {
            println!();
            println!("Application deployed, but Spin could not establish readiness");
            match destination {
                Destination::Cloud(url) => {
                    println!(
                        "Check the Fermyon Cloud dashboard to see the application status: {url}"
                    );
                }
            }
            return;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[instrument(level = "debug")]
async fn is_ready(app_info_url: &str, expected_version: &str) -> Result<bool> {
    // If the request fails, we assume the app isn't ready
    let resp = match reqwest::get(app_info_url).await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!("Readiness check failed: {err:?}");
            return Ok(false);
        }
    };
    // If the response status isn't success, the app isn't ready
    if !resp.status().is_success() {
        tracing::debug!("App not ready: {}", resp.status());
        return Ok(false);
    }
    // If the app was previously deployed then it will have an outdated
    // version, in which case the app isn't ready
    if let Ok(app_info) = resp.json::<AppInfo>().await {
        let active_version = app_info.oci_image_digest;
        if active_version.as_deref() != Some(expected_version) {
            tracing::debug!("Active version {active_version:?} != expected {expected_version:?}");
            return Ok(false);
        }
    }
    Ok(true)
}

fn print_available_routes(
    app: &DeployableApp,
    app_name: &str,
    app_base_url: &Url,
    base: &str,
    router: &Router,
) {
    // Strip any trailing slash from base URL
    let app_base_url = app_base_url.to_string();
    let route_prefix = app_base_url.strip_suffix('/').unwrap_or(&app_base_url);

    // Ensure base starts with a /
    let base = if !base.starts_with('/') {
        format!("/{base}")
    } else {
        base.to_owned()
    };

    let app_root_url = format!("{route_prefix}{base}");
    let admin_url = format!("{}app/{app_name}", DEFAULT_CLOUD_URL); // URL already has scheme and /

    println!();
    println!("View application:   {app_root_url}");

    if router
        .routes()
        .any(|(route, _)| route.to_string() != " (wildcard)")
    {
        println!("  Routes:");
        for (route, component_id) in router.routes() {
            println!("  - {}: {}{}", component_id, route_prefix, route);
            if let Some(description) = app.component_description(component_id) {
                println!("    {}", description);
            }
        }
    }

    println!("Manage application: {admin_url}");
}

// Check if the token has expired - or is so close to expiring that we
// aren't confident it will last long enough to complete a deploy!
// If the expiration is None, assume the token is current and will last long enough.
fn needs_renewal(login_connection: &LoginConnection) -> Result<bool> {
    match &login_connection.expiration {
        Some(expiration) => match DateTime::parse_from_rfc3339(expiration) {
            Ok(time) => {
                let time = time.to_utc();
                let token_time_remaining = time - Utc::now();
                Ok(token_time_remaining < TOKEN_MUST_HAVE_REMAINING)
            }
            Err(err) => Err(anyhow!(
                "Failed to parse token expiration time '{}'. Error: {}",
                expiration,
                err
            )),
        },
        None => Ok(false),
    }
}

pub async fn login_connection(deployment_env_id: Option<&str>) -> Result<LoginConnection> {
    let path = config_file_path(deployment_env_id)?;

    // log in if config.json does not exist or cannot be read
    let data = match fs::read_to_string(path.clone()).await {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match deployment_env_id {
                Some(name) => {
                    // TODO: allow auto redirect to login preserving the name
                    eprintln!("You have no instance saved as '{}'", name);
                    eprintln!("Run `spin login --environment-name {}` to log in", name);
                    std::process::exit(1);
                }
                None => {
                    // log in, then read config
                    // TODO: propagate deployment id (or bail if nondefault?)
                    LoginCommand::parse_from(vec!["login"]).run().await?;
                    fs::read_to_string(path.clone()).await?
                }
            }
        }
        Err(e) => {
            bail!("Could not log in: {}", e);
        }
    };

    let mut login_connection: LoginConnection = serde_json::from_str(&data)?;
    let expired = match needs_renewal(&login_connection) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("{}\n", err);
            eprintln!("Run `spin login` to log in again");
            std::process::exit(1);
        }
    };

    if expired {
        // if we have a refresh token available, let's try to refresh the token
        match login_connection.refresh_token {
            Some(refresh_token) => {
                // Only Cloud has support for refresh tokens
                let connection_config = ConnectionConfig {
                    url: login_connection.url.to_string(),
                    insecure: login_connection.danger_accept_invalid_certs,
                    token: login_connection.token.clone(),
                };
                let client = CloudClient::new(connection_config.clone());

                match client
                    .refresh_token(login_connection.token, refresh_token)
                    .await
                {
                    Ok(token_info) => {
                        login_connection.token = token_info.token;
                        login_connection.refresh_token = Some(token_info.refresh_token);
                        login_connection.expiration = Some(token_info.expiration);
                        // save new token info
                        let path = config_file_path(deployment_env_id)?;
                        std::fs::write(path, serde_json::to_string_pretty(&login_connection)?)?;
                    }
                    Err(e) => {
                        eprintln!("Failed to refresh token: {}", e);
                        match deployment_env_id {
                            Some(name) => {
                                eprintln!(
                                    "Run `spin login --environment-name {}` to log in again",
                                    name
                                );
                            }
                            None => {
                                eprintln!("Run `spin login` to log in again");
                            }
                        }
                        std::process::exit(1);
                    }
                }
            }
            None => {
                // session has expired and we have no way to refresh the token - log back in
                match deployment_env_id {
                    Some(name) => {
                        // TODO: allow auto redirect to login preserving the name
                        eprintln!("Your login to this environment has expired");
                        eprintln!(
                            "Run `spin login --environment-name {}` to log in again",
                            name
                        );
                        std::process::exit(1);
                    }
                    None => {
                        LoginCommand::parse_from(vec!["login"]).run().await?;
                        let new_data = fs::read_to_string(path.clone()).await.context(format!(
                            "Cannot find spin config at {}",
                            path.to_string_lossy()
                        ))?;
                        login_connection = serde_json::from_str(&new_data)?;
                    }
                }
            }
        }
    }

    Ok(login_connection)
}

// TODO: unify with login
pub fn config_file_path(deployment_env_id: Option<&str>) -> Result<PathBuf> {
    let root = dirs::config_dir()
        .context("Cannot find configuration directory")?
        .join("fermyon");

    let file_stem = match deployment_env_id {
        None => "config",
        Some(id) => id,
    };
    let file = format!("{}.json", file_stem);

    let path = root.join(file);

    Ok(path)
}

fn parse_linkage_specs(links: &[impl AsRef<str>]) -> anyhow::Result<resource::Scripted> {
    // TODO: would this be nicer as a fold?
    let mut strategy = resource::Scripted::default();

    for link in links.iter().map(|s| s.as_ref().parse::<LinkageSpec>()) {
        let link = link?;
        strategy.set_label_action(&link.label, link.resource_name, link.resource_type)?;
    }
    Ok(strategy)
}

struct LinkageSpec {
    label: String,
    resource_name: String,
    resource_type: ResourceType,
}

impl LinkageSpec {
    fn new(label: String, resource_name: String, resource_type: ResourceType) -> Self {
        LinkageSpec {
            label,
            resource_name,
            resource_type,
        }
    }
}

impl FromStr for LinkageSpec {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let Some((resource_str, pair)) = s.split_once(':') else {
            bail!("Links must be of the form 'sqlite:label=database' or 'kv:label=store'");
        };

        let Some((label, resource)) = pair.split_once('=') else {
            bail!("Links must be of the form 'sqlite:label=database' or 'kv:label=store'");
        };

        let label = label.trim();
        let resource = resource.trim();

        match resource_str {
            "sqlite" => Ok(LinkageSpec {
                label: label.to_owned(),
                resource_name: resource.to_owned(),
                resource_type: ResourceType::Database,
            }),
            "kv" => Ok(LinkageSpec {
                label: label.to_owned(),
                resource_name: resource.to_owned(),
                resource_type: ResourceType::KeyValueStore,
            }),
            _ => bail!("Links must be of the form 'sqlite:label=database' or 'kv:label=store'"),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn accepts_only_valid_app_names() {
        check_safe_app_name("hello").expect("should have accepted 'hello'");
        check_safe_app_name("hello-world").expect("should have accepted 'hello-world'");
        check_safe_app_name("hell0_w0rld").expect("should have accepted 'hell0_w0rld'");

        let _ =
            check_safe_app_name("hello/world").expect_err("should not have accepted 'hello/world'");

        let _ =
            check_safe_app_name("hello world").expect_err("should not have accepted 'hello world'");
    }

    #[test]
    fn should_sanitize_app_name() {
        assert_eq!("hello-world", sanitize_app_name("hello-world"));
        assert_eq!("hello-world2000", sanitize_app_name("Hello-World2000"));
        assert_eq!("hello-world", sanitize_app_name(".-_hello-world_-"));
        assert_eq!("hello-world", sanitize_app_name(" hello -world "));
    }

    #[test]
    fn should_sanitize_app_version() {
        assert_eq!("v0.1.0", sanitize_app_version("v0.1.0"));
        assert_eq!("_v0.1.0", sanitize_app_version("_v0.1.0"));
        assert_eq!("v0.1.0_-", sanitize_app_version(".-v0.1.0_-"));
        assert_eq!("v0.1.0", sanitize_app_version(" v 0.1.0 "));
        assert_eq!(
            "v0.1.0+Hello-World",
            sanitize_app_version(" v 0.1.0+Hello-World ")
        );
        assert_eq!(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            sanitize_app_version("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b")
        );
    }

    fn deploy_cmd_for_test_file(filename: &str) -> DeployCommand {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(filename);
        DeployCommand {
            app_source: None,
            file_source: Some(path),
            registry_source: None,
            build: false,
            readiness_timeout_secs: 60,
            deployment_env_id: None,
            key_values: vec![],
            variables: vec![],
            links: vec![],
        }
    }

    fn get_trigger_base(mut app: DeployableApp) -> String {
        let serde_json::map::Entry::Occupied(trigger) = app.0.metadata.entry("trigger") else {
            panic!("Expected trigger metadata but entry was vacant");
        };
        let base = trigger
            .get()
            .as_object()
            .unwrap()
            .get("base")
            .expect("Manifest should have had a base but didn't");
        base.as_str()
            .expect("HTTP base should have been a string but wasn't")
            .to_owned()
    }

    #[tokio::test]
    async fn if_http_base_is_set_then_it_is_respected() {
        let temp_dir = tempfile::tempdir().unwrap();

        let cmd = deploy_cmd_for_test_file("based_v1.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/base", base);

        let cmd = deploy_cmd_for_test_file("based_v2.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/base", base);
    }

    #[tokio::test]
    async fn if_http_base_is_not_set_then_it_is_inserted() {
        let temp_dir = tempfile::tempdir().unwrap();

        let cmd = deploy_cmd_for_test_file("unbased_v1.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/", base);

        let cmd = deploy_cmd_for_test_file("unbased_v2.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/", base);
    }

    #[tokio::test]
    async fn plugin_version_should_be_set() {
        let temp_dir = tempfile::tempdir().unwrap();

        let cmd = deploy_cmd_for_test_file("minimal_v2.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let version = app.0.metadata.get("cloud_plugin_version").unwrap();
        assert_eq!(crate::VERSION, version);
    }

    fn string_set(strs: &[&str]) -> HashSet<String> {
        strs.iter().map(|s| s.to_string()).collect()
    }

    #[tokio::test]
    async fn new_app_databases_are_created_and_linked() {
        let db_labels = string_set(&["default", "finance"]);
        let links = ["sqlite:default=def-o-rama", "sqlite:finance=excel"];
        let linkages = parse_linkage_specs(&links).unwrap();

        let mut client = cloud::MockCloudClientInterface::new();

        client.expect_get_databases().returning(|_| Ok(vec![]));
        client
            .expect_create_database()
            .withf(|db, rlabel| db == "def-o-rama" && rlabel.is_none())
            .returning(move |_, _| Ok(()));
        client.expect_get_databases().returning(|_| Ok(vec![]));
        client
            .expect_create_database()
            .withf(|db, rlabel| db == "excel" && rlabel.is_none())
            .returning(|_, _| Ok(()));

        let databases_to_link = resource::create_resources_for_new_app(
            &client,
            "test:script-new-app",
            db_labels,
            HashSet::new(),
            &linkages,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(2, databases_to_link.len());

        client
            .expect_create_database_link()
            .withf(move |db, rlabel| db == "def-o-rama" && rlabel.label == "default")
            .returning(|_, _| Ok(()));
        client
            .expect_create_database_link()
            .withf(|db, rlabel| db == "excel" && rlabel.label == "finance")
            .returning(|_, _| Ok(()));

        resource::link_resources(
            &client,
            "test:script-new-app",
            uuid::Uuid::new_v4(),
            databases_to_link,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn new_app_kv_stores_are_created_and_linked() {
        let kv_labels = string_set(&["default", "finance"]);
        let links = [
            "sqlite:default=sqldb",
            "sqlite:finance=sqldb2",
            "kv:default=def-o-rama",
            "kv:finance=excel",
        ];
        let linkages = parse_linkage_specs(&links).unwrap();

        let mut client = cloud::MockCloudClientInterface::new();

        client
            .expect_get_key_value_stores()
            .returning(|_| Ok(vec![]));
        client
            .expect_create_key_value_store()
            .withf(|s, rlabel| s == "def-o-rama" && rlabel.is_none())
            .returning(move |_, _| Ok(()));
        client
            .expect_get_key_value_stores()
            .returning(|_| Ok(vec![]));
        client
            .expect_create_key_value_store()
            .withf(|s, rlabel| s == "excel" && rlabel.is_none())
            .returning(|_, _| Ok(()));

        let stores_to_link = resource::create_resources_for_new_app(
            &client,
            "test:script-new-app",
            HashSet::new(),
            kv_labels,
            &linkages,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(2, stores_to_link.len());

        client
            .expect_create_key_value_store_link()
            .withf(move |db, rlabel| db == "def-o-rama" && rlabel.label == "default")
            .returning(|_, _| Ok(()));
        client
            .expect_create_key_value_store_link()
            .withf(|db, rlabel| db == "excel" && rlabel.label == "finance")
            .returning(|_, _| Ok(()));

        resource::link_resources(
            &client,
            "test:script-new-app",
            uuid::Uuid::new_v4(),
            stores_to_link,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn new_app_kv_stores_and_databases_are_created_and_linked() {
        let kv_labels = string_set(&["default", "finance"]);
        let db_labels = string_set(&["default", "finance"]);
        let links = [
            "sqlite:default=def-o-rama",
            "sqlite:finance=excel",
            "kv:default=def-o-rama",
            "kv:finance=excel",
        ];
        let linkages = parse_linkage_specs(&links).unwrap();

        let mut client = cloud::MockCloudClientInterface::new();

        client
            .expect_get_key_value_stores()
            .returning(|_| Ok(vec![]));
        client
            .expect_create_key_value_store()
            .withf(|s, rlabel| s == "def-o-rama" && rlabel.is_none())
            .returning(move |_, _| Ok(()));
        client
            .expect_get_key_value_stores()
            .returning(|_| Ok(vec![]));
        client
            .expect_create_key_value_store()
            .withf(|s, rlabel| s == "excel" && rlabel.is_none())
            .returning(|_, _| Ok(()));
        client.expect_get_databases().returning(|_| Ok(vec![]));
        client
            .expect_create_database()
            .withf(|db, rlabel| db == "def-o-rama" && rlabel.is_none())
            .returning(move |_, _| Ok(()));
        client.expect_get_databases().returning(|_| Ok(vec![]));
        client
            .expect_create_database()
            .withf(|db, rlabel| db == "excel" && rlabel.is_none())
            .returning(|_, _| Ok(()));

        let stores_to_link = resource::create_resources_for_new_app(
            &client,
            "test:script-new-app",
            db_labels,
            kv_labels,
            &linkages,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(4, stores_to_link.len());

        client
            .expect_create_key_value_store_link()
            .withf(move |db, rlabel| db == "def-o-rama" && rlabel.label == "default")
            .returning(|_, _| Ok(()));
        client
            .expect_create_key_value_store_link()
            .withf(|db, rlabel| db == "excel" && rlabel.label == "finance")
            .returning(|_, _| Ok(()));
        client
            .expect_create_database_link()
            .withf(move |db, rlabel| db == "def-o-rama" && rlabel.label == "default")
            .returning(|_, _| Ok(()));
        client
            .expect_create_database_link()
            .withf(|db, rlabel| db == "excel" && rlabel.label == "finance")
            .returning(|_, _| Ok(()));

        resource::link_resources(
            &client,
            "test:script-new-app",
            uuid::Uuid::new_v4(),
            stores_to_link,
        )
        .await
        .unwrap();
    }
}
