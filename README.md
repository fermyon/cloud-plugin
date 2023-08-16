# Fermyon Cloud Plugin

A [Spin plugin](https://github.com/fermyon/spin-plugins) for interacting with Fermyon Cloud from the [Spin CLI](https://github.com/fermyon/spin).

## Installing the latest plugin

```sh
spin plugin install --url https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
```

## Building and installing local changes

1. Build the plugin.

    ```sh
    make build
    ```

1. Install the plugin.

    ```sh
    make install
    ```

1. Run the plugin.

    ```sh
    spin cloud --help
    ```

## Run tests

1. Lint/format code.

    ```sh
    make lint
    ```

1. Run tests.

    ```sh
    make test
    ```

## Installing local changes by hand

In case `make install` doesn't work or you prefer to install manually.

1. Get the manifest.

    ```sh
    curl -LRO https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
    ```

1. Update the manifest to modify the `url` field to point to the path to local package (i.e. `"url": "file:///path/to/cloud-plugin/cloud.tar.gz"`) and update the shasum.

1. Install the plugin, pointing to the path to the manifest.

    ```sh
    spin plugin install -f ./cloud.json
    ```
