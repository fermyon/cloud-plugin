fn main() {
    vergen::EmitBuilder::builder()
        .git_commit_date()
        .git_sha(true)
        .emit()
        .expect("failed to extract build information");
}
