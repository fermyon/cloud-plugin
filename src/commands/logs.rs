use std::ops::Sub;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::Utc;
use cloud::{CloudClientExt, CloudClientInterface};
use cloud_openapi::models::Entry;
use std::option::Option;

use crate::commands::create_cloud_client;
use crate::opts::*;
use clap::Parser;
use uuid::Uuid;

/// fetch logs for an app from Fermyon Cloud
#[derive(Parser, Debug)]
pub struct LogsCommand {
    /// Use the Fermyon instance saved under the specified name.
    /// If omitted, Spin looks for app in default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV,
        hidden = true
    )]
    pub deployment_env_id: Option<String>,

    /// App name
    pub app: String,

    /// Follow logs output
    #[clap(name = "follow", long = "follow")]
    pub follow: bool,

    /// Number of lines to show from the end of the logs
    #[clap(name = "tail", long = "tail", default_value = "10")]
    pub max_lines: i32,

    /// Interval in seconds to refresh logs from cloud
    #[clap(parse(try_from_str = parse_interval), name="interval", long="interval", default_value = "2")]
    pub interval_secs: std::time::Duration,

    /// Only return logs newer than a relative duration. The duration format is a number
    /// and a unit, where the unit is 's' for seconds, 'm' for minutes, 'h' for hours
    /// or 'd' for days (e.g. "30m" for 30 minutes ago).  The default is 7 days.
    #[clap(parse(try_from_str = parse_duration), name="since", long="since", default_value = "7d")]
    pub since: std::time::Duration,

    /// Show timestamps
    #[clap(
        name = "show-timestamps",
        long = "show-timestamps",
        default_value = "true",
        action = clap::ArgAction::Set
    )]
    pub show_timestamp: bool,
}

impl LogsCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.deployment_env_id.as_deref()).await?;
        self.logs(&client).await
    }

    async fn logs(self, client: &impl CloudClientInterface) -> Result<()> {
        let app_id = client
            .get_app_id(&self.app)
            .await
            .with_context(|| format!("failed to find app with name {:?}", &self.app))?
            .with_context(|| format!("app with name {:?} not found", &self.app))?;

        fetch_logs_and_print_loop(
            client,
            app_id,
            self.follow,
            self.interval_secs,
            self.max_lines,
            self.since,
            self.show_timestamp,
        )
        .await?;

        Ok(())
    }
}

async fn fetch_logs_and_print_loop(
    client: &impl CloudClientInterface,
    app_id: Uuid,
    follow: bool,
    interval: Duration,
    max_lines: i32,
    since: Duration,
    show_timestamp: bool,
) -> Result<()> {
    let mut curr_since = Utc::now().sub(since).to_rfc3339();
    curr_since =
        fetch_logs_and_print_once(client, app_id, Some(max_lines), curr_since, show_timestamp)
            .await?;

    if !follow {
        return Ok(());
    }

    loop {
        tokio::time::sleep(interval).await;
        curr_since =
            fetch_logs_and_print_once(client, app_id, None, curr_since, show_timestamp).await?;
    }
}

async fn fetch_logs_and_print_once(
    client: &impl CloudClientInterface,
    app_id: Uuid,
    max_lines: Option<i32>,
    since: String,
    show_timestamp: bool,
) -> Result<String> {
    let entries = client
        .app_logs_raw(app_id.to_string(), max_lines, Some(since.to_string()))
        .await?
        .entries;

    if entries.is_empty() {
        return Ok(since.to_owned());
    }

    let updated_since = print_logs(&entries, show_timestamp);
    if let Some(u) = updated_since {
        return Ok(u.to_owned());
    }

    Ok(since)
}

fn print_logs(entries: &[Entry], show_timestamp: bool) -> Option<&str> {
    let mut since = None;
    for entry in entries.iter().rev() {
        let Some(log_lines) = entry.log_lines.as_ref() else {
            continue;
        };

        for log_entry in log_lines {
            let Some(log) = log_entry.line.as_ref() else {
                continue;
            };

            if let Some(time) = &log_entry.time {
                if show_timestamp {
                    println!("[{time}] {log}");
                } else {
                    println!("{log}");
                }
                since = Some(time.as_str());
            }
        }
    }

    since
}

fn parse_duration(arg: &str) -> anyhow::Result<std::time::Duration> {
    let duration = if let Some(parg) = arg.strip_suffix('s') {
        let value = parg.parse()?;
        std::time::Duration::from_secs(value)
    } else if let Some(parg) = arg.strip_suffix('m') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 60)
    } else if let Some(parg) = arg.strip_suffix('h') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 60 * 60)
    } else if let Some(parg) = arg.strip_suffix('d') {
        let value: u64 = parg.parse()?;
        std::time::Duration::from_secs(value * 24 * 60 * 60)
    } else {
        bail!(r#"since must be a number followed by an allowed unit ("300s", "5m", "4h" or "1d")"#);
    };

    Ok(duration)
}

fn parse_interval(arg: &str) -> anyhow::Result<std::time::Duration> {
    let value = arg.parse()?;
    if value < 2 {
        bail!("interval cannot be less than 2 seconds")
    }

    Ok(std::time::Duration::from_secs(value))
}
