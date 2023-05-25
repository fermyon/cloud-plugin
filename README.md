# Fermyon Cloud Plugin

A [Spin plugin](https://github.com/fermyon/spin-plugins) for interacting with Fermyon Cloud from the [Spin CLI](https://github.com/fermyon/spin).

## Installing the latest plugin

```sh
spin plugin install --url https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
```

## Building and installing local changes

1. Package the plugin.

    ```sh
    cargo build --release
    cp target/release/cloud-plugin cloud
    tar -czvf cloud.tar.gz cloud
    sha256sum cloud.tar.gz
    rm cloud
    # Outputs a shasum to add to cloud.json
    ```

1. Get the manifest.

    ```sh
    curl -LRO https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
    ```

1. Update the manifest to modify the `url` field to point to the path to local package (i.e. `"url": "file:///path/to/cloud-plugin/plugin/cloud.tar.gz"`) and update the shasum.

1. Install the plugin, pointing to the path to the manifest.

    ```sh
    spin plugin install -f ./plugin/cloud.json
    ```

1. Run the plugin.

    ```sh
    spin cloud --help
    ```
