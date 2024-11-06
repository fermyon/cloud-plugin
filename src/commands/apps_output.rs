use std::fmt::Display;

use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, ValueEnum, PartialEq, Clone)]
pub(crate) enum OutputFormat {
    Plain,
    Json,
}

#[derive(Serialize)]
pub(crate) struct AppInfo {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    url: Option<String>,
    #[serde(rename = "domainInfo")]
    domain_info: DomainInfo,
}

#[derive(Serialize)]
pub(crate) struct DomainInfo {
    domain: Option<String>,
    #[serde(rename = "validationFinished")]
    validation_finished: bool,
}

impl AppInfo {
    pub(crate) fn new(
        name: String,
        description: Option<String>,
        domain: Option<String>,
        domain_validation_finished: bool,
    ) -> Self {
        let url = domain.as_ref().map(|d| format!("https://{}", d));
        let desc: Option<String> = match description {
            Some(d) => match d.is_empty() {
                true => None,
                false => Some(d),
            },
            None => None,
        };
        Self {
            name,
            description: desc,
            url,
            domain_info: DomainInfo {
                domain,
                validation_finished: domain_validation_finished,
            },
        }
    }
}

impl Display for AppInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Name: {}", self.name)?;
        if self
            .description
            .as_ref()
            .is_some_and(|desc| !desc.is_empty())
        {
            writeln!(f, "Description: {}", self.description.clone().unwrap())?;
        }
        if self.domain_info.domain.is_some() {
            let domain = self.domain_info.domain.clone().unwrap();
            writeln!(f, "URL: https://{}", domain)?;
            if !self.domain_info.validation_finished {
                writeln!(f, "Validation for {} is in progress", domain)?;
            };
        }
        Ok(())
    }
}

pub(crate) fn print_app_list(apps: Vec<String>, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&apps).unwrap()),
        OutputFormat::Plain => println!("{}", apps.join("\n")),
    }
}

pub(crate) fn print_app_info(app: AppInfo, format: OutputFormat) {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&app).unwrap()),
        OutputFormat::Plain => print!("{}", app),
    }
}
