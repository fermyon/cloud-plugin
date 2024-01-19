/// Module for determining the linkable resource to target for a command
use crate::commands::links_output::{ResourceLinks, ResourceType};

#[derive(Debug, PartialEq)]
pub enum ResourceTarget {
    ByName(String),
    ByLabel { label: String, app: String },
}

impl ResourceTarget {
    pub fn find_in(
        &self,
        resources: Vec<ResourceLinks>,
        resource_type: ResourceType,
    ) -> anyhow::Result<ResourceLinks> {
        match self {
            Self::ByName(resource) => resources
                .into_iter()
                .find(|r| &r.name == resource)
                .ok_or_else(|| {
                    anyhow::anyhow!("No {resource_type} found with name \"{resource}\"")
                }),
            Self::ByLabel { label, app } => resources
                .into_iter()
                .find(|r| r.has_link(label, Some(app.as_str())))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        r#"No {resource_type} found with label "{label}" for app "{app}""#
                    )
                }),
        }
    }

    pub fn from_inputs(
        resource: &Option<String>,
        label: &Option<String>,
        app: &Option<String>,
    ) -> anyhow::Result<ResourceTarget> {
        match (resource, label, app) {
            (Some(r), None, None) => Ok(ResourceTarget::ByName(r.to_owned())),
            (None, Some(l), Some(a)) => Ok(ResourceTarget::ByLabel {
                label: l.to_owned(),
                app: a.to_owned(),
            }),
            _ => Err(anyhow::anyhow!("Invalid combination of arguments")), // Should be prevented by clap
        }
    }
}

#[cfg(test)]
mod test {
    use cloud_openapi::models::ResourceLabel;
    use uuid::Uuid;

    use super::*;

    #[test]
    fn test_execute_target_from_inputs() {
        assert_eq!(
            ResourceTarget::from_inputs(&Some("mykv".to_owned()), &None, &None).unwrap(),
            ResourceTarget::ByName("mykv".to_owned())
        );
        assert_eq!(
            ResourceTarget::from_inputs(&None, &Some("label".to_owned()), &Some("app".to_owned()))
                .unwrap(),
            ResourceTarget::ByLabel {
                label: "label".to_owned(),
                app: "app".to_owned(),
            }
        );
        assert!(ResourceTarget::from_inputs(&None, &None, &None).is_err());
        assert!(ResourceTarget::from_inputs(
            &Some("mykv".to_owned()),
            &Some("label".to_owned()),
            &Some("app".to_owned())
        )
        .is_err());
    }

    #[test]
    fn test_execute_target_find_in() {
        let links = vec![ResourceLabel {
            app_id: Uuid::new_v4(),
            app_name: Some("app".to_owned()),
            label: "label".to_owned(),
        }];
        let rl1 = ResourceLinks::new("mykv".to_owned(), vec![]);
        let rl2 = ResourceLinks::new("mykv2".to_owned(), links);
        let resources = vec![rl1.clone(), rl2.clone()];
        assert_eq!(
            ResourceTarget::ByName("mykv".to_owned())
                .find_in(resources.clone(), ResourceType::KeyValueStore)
                .unwrap(),
            rl1
        );
        assert_eq!(
            ResourceTarget::ByLabel {
                label: "label".to_owned(),
                app: "app".to_owned(),
            }
            .find_in(resources.clone(), ResourceType::KeyValueStore)
            .unwrap(),
            rl2
        );
        assert!(ResourceTarget::ByName("foo".to_owned())
            .find_in(resources.clone(), ResourceType::KeyValueStore)
            .is_err());
        assert!(ResourceTarget::ByLabel {
            label: "foo".to_owned(),
            app: "app".to_owned(),
        }
        .find_in(resources.clone(), ResourceType::KeyValueStore)
        .is_err());
    }
}
