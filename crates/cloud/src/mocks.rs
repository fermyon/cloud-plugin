#[derive(Clone)]
pub struct Link {
    pub name: String,
    pub app: String,
}

pub struct Database {
    pub name: String,
    pub links: Vec<Link>,
}

impl Database {
    pub fn new(name: String, links: Vec<Link>) -> Database {
        Database { name, links }
    }
}

impl Link {
    pub fn new(name: String, app: String) -> Self {
        Link { name, app }
    }
}

pub fn mock_databases_list() -> Vec<Database> {
    let db1_links = vec![
        Link::new("foo".to_string(), "app1".to_string()),
        Link::new("yee".to_string(), "app2".to_string()),
    ];
    let db2_links = vec![Link::new("bar".to_string(), "app1".to_string())];
    vec![
        Database::new("db1".to_string(), db1_links),
        Database::new("db2".to_string(), db2_links),
        Database::new("db3".to_string(), vec![]),
    ]
}
