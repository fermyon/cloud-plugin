use cloud_openapi::models::{Database, ResourceLabel};
use uuid::Uuid;

// TODO: remove all of this mocking code
fn create_mock_resource_label(label: String) -> ResourceLabel {
    let app_id = Uuid::new_v4();
    let app_name = format!("myapp-{}", app_id.as_simple());
    ResourceLabel {
        label,
        app_id,
        app_name: Some(Some(app_name)),
    }
}

pub fn mock_databases_list() -> Vec<Database> {
    let db1_links = vec![
        create_mock_resource_label("foo".to_string()),
        create_mock_resource_label("yee".to_string()),
    ];
    let db2_links = vec![create_mock_resource_label("bar".to_string())];
    vec![
        Database::new("db1".to_string(), db1_links),
        Database::new("db2".to_string(), db2_links),
        Database::new("db3".to_string(), vec![]),
    ]
}
