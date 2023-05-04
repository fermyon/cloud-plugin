# Plugin Manifest and Artifact

This is a temporary directory for the plugin binary and manifest.
Later, these should be moved to a release artifact the [plugins repository](https://github.com/fermyon/spin-plugins), respectively.

## Trying out the Fermyon Cloud plugin

1. Update the manifest to replace `path/to` with the absolute path to this repository.
2. Install the plugin and run it

```sh
spin plugin install -f ./plugin/cloud-fermyon.json
spin cloud-fermyon login
spin cloud-fermyon deploy
```

## Repackaging the plugin

```sh
cargo build --release
cp target/release/cloud-plugin cloud-fermyon
tar -czvf cloud.tar.gz cloud-fermyon
sha256sum cloud.tar.gz
rm cloud-fermyon
# Update cloud-fermyon.json with shasum
```
