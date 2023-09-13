use uuid::Uuid;

#[derive(Clone, PartialEq)]
pub struct Link {
    pub label: String,
    pub app_id: Uuid,
}

pub struct Database {
    pub name: String,
    pub links: Vec<Link>,
}

impl Database {
    pub fn has_label(&self, label: &str) -> bool {
        self.links.iter().any(|l| l.label == label)
    }
}

impl Database {
    pub fn new(name: String, links: Vec<Link>) -> Database {
        Database { name, links }
    }
}

impl Link {
    pub fn new(label: String) -> Self {
        let app_id = Uuid::new_v4();
        Link { label, app_id }
    }
}

pub fn mock_databases_list() -> Vec<Database> {
    let db1_links = vec![Link::new("foo".to_string()), Link::new("yee".to_string())];
    let db2_links = vec![Link::new("bar".to_string())];
    vec![
        Database::new("db1".to_string(), db1_links),
        Database::new("db2".to_string(), db2_links),
        Database::new("db3".to_string(), vec![]),
    ]
}
