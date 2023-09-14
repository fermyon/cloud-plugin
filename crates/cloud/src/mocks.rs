use uuid::Uuid;

/// A label representing a resource for a given app
#[derive(Clone, PartialEq)]
pub struct AppLabel {
    pub label: String,
    pub app_id: Uuid,
    pub app_name: String,
}

#[derive(Clone, PartialEq)]
pub struct DatabaseLink {
    pub app_label: AppLabel,
    pub database: String,
}

impl DatabaseLink {
    pub fn new(app_label: AppLabel, database: String) -> Self {
        Self {
            app_label,
            database,
        }
    }

    pub fn has_label(&self, label: &str) -> bool {
        self.app_label.label == label
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

// TODO: remove all of this mocking code
impl AppLabel {
    fn new(label: String) -> Self {
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

pub fn mock_links_list() -> Vec<DatabaseLink> {
    vec![
        DatabaseLink::new(AppLabel::new("foo".to_string()), "db1".to_string()),
        DatabaseLink::new(AppLabel::new("yee".to_string()), "db2".to_string()),
    ]
}
