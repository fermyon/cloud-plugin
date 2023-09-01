#[derive(Clone)]
pub struct Link {
    pub name: String,
    pub app: String,
    pub database: String,
}

pub struct Database {
    pub name: String,
    pub links: Vec<Link>,
}

impl Database {
    pub fn new(name: &str, links: Vec<Link>) -> Database {
        Database {
            name: name.to_string(),
            links,
        }
    }
}

impl Link {
    pub fn new(name: &str, app: &str, database: &str) -> Self {
        Link {
            name: name.to_string(),
            app: app.to_string(),
            database: database.to_string(),
        }
    }
}

pub fn mock_databases_list() -> Vec<Database> {
    let db1_links = vec![
        Link::new("foo", "app1", "db1"),
        Link::new("yee", "app2", "db1"),
    ];
    let db2_links = vec![Link::new("bar", "app1", "db2")];
    vec![
        Database::new("db1", db1_links),
        Database::new("db2", db2_links),
        Database::new("db3", vec![]),
    ]
}
