use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub struct AppLabel {
    pub label: String,
    pub app_id: Uuid,
    pub app_name: String,
}

#[derive(Clone, PartialEq)]
pub struct Link {
    pub app_label: AppLabel,
    pub database: String,
}

impl Link {
    fn new(app_label: AppLabel, database: String) -> Self {
        Self {
            app_label,
            database,
        }
    }
}

pub struct Database {
    pub name: String,
    pub links: Vec<AppLabel>,
}

impl Database {
    pub fn has_label(&self, label: &str) -> bool {
        self.links.iter().any(|l| l.label == label)
    }

    pub fn has_link(&self, link: &AppLabel) -> bool {
        self.links.iter().any(|l| l == link)
    }
}

impl Database {
    pub fn new(name: String, links: Vec<AppLabel>) -> Database {
        Database { name, links }
    }
}

impl AppLabel {
    pub fn new(label: String) -> Self {
        let app_id = Uuid::new_v4();
        let app_name = format!("myapp-{}", app_id.as_simple());
        AppLabel {
            label,
            app_id,
            app_name,
        }
    }
}

pub fn mock_databases_list() -> Vec<Database> {
    let db1_links = vec![
        AppLabel::new("foo".to_string()),
        AppLabel::new("yee".to_string()),
    ];
    let db2_links = vec![AppLabel::new("bar".to_string())];
    vec![
        Database::new("db1".to_string(), db1_links),
        Database::new("db2".to_string(), db2_links),
        Database::new("db3".to_string(), vec![]),
    ]
}

pub fn mock_links_list() -> Vec<Link> {
    vec![
        Link::new(AppLabel::new("foo".to_string()), "db1".to_string()),
        Link::new(AppLabel::new("yee".to_string()), "db2".to_string()),
    ]
}
