fn main() {
    vergen::EmitBuilder::builder()
        .git_commit_date()
        .git_sha(true)
        .fail_on_error()
        .emit()
        .expect("failed to extract build information");
}
