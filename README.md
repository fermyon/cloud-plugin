# Fermyon Cloud Plugin

A [Spin plugin](https://github.com/fermyon/spin-plugins) for interacting with Fermyon Cloud from the [Spin CLI](https://github.com/fermyon/spin).

## Installing the latest version of the plugin

The latest stable release of the cloud plugin can be installed like so:

```sh
spin plugins update
spin plugin install cloud
```

## Installing the canary version of the plugin

The canary release of the cloud plugin represents the most recent commits on `main` and may not be stable, with some features still in progress.

```sh
spin plugin install --url https://github.com/fermyon/cloud-plugin/releases/download/canary/cloud.json
```

## Building and installing local changes

1. Install `spin pluginify`

    ```sh
    spin plugins update
    spin plugins install pluginify --yes
    ```

2. Build, package and install the plugin.

    ```sh
    cargo build --release
    spin pluginify --install
    ```

3. Run the plugin.

    ```sh
    spin cloud --help
    ```
