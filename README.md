# ROSE Online Updater

The ROSE Online Updater project is a collection of tools designed to create and
deliver game updates to the player.

## Overview

At a high level, the updater works by creating "archive" files for all the files
that need to be distributed to the user. These archives are created using the
`rose-updater-archive` tool.

Once the archives are created, they can be uploaded to any location and served
with a standard HTTP web server that supports HTTP Range headers.

Stored with the archive data is a "remote manifest". This is a file that stores
some information such as the list of files available for download and their hash.

A user uses the `rose-updater` tool to update their local game by pointing to
the URL of the archive (default: https://updates.roseonlinegame.com). The
updater will then download the remote manifest and check to see which files need
to be updated.

First, the updater will check if it has any information cached about the local
files in a "local manifest" in the user's app cache directory (e.g.
`%LocalAppData%\Rednim Games\ROSE Online\cache\updater\default\local_manifest.json`)

It will compare the cached hashes with the remote hash and only update the files
that have a different remote hash. For new files or files that have been
modified, it will begin a clone process which will only clone the differences
between the two files. If there is no local manifest then it will download all
the remote files.

In the case where the updater itself needs to be updated, it will first update
only itself and then restart its new version to continue the update process.

Once all the files have been synced, the local manifest will be updated at the
cached location.

Finally, the updater will launch the actual game after completing all the steps.

For more information check use the `--help` commands the executables

## Features

### Force rechecks

It's possible the cache might get out of sync or we might want to force our
client to be updated. In this case we can take advantage of two flags to help
force a sync: `--force-recheck` and `--force-recheck-updater`. This will force
the updater to ignore the local cache and recheck all the files and the updater
respectively.

### Multiple Clients

The updater can be used to update multiple different versions of the game
because each local manifest is namespaced by the input url. This ensures that
the cached information is stored distinctly for each remote target.
Additionally, `--url` and other flags can be used to change the
default behavior.

For example, consider this directory structure:

```
| - rose_production/
| --- 3ddata/
| --- trose.exe
| --- rose-updater.exe
| - rose_development/
| --- 3ddata/
| --- trose.exe
| --- rose-updater.exe
```

These two directories can be independently updated using some combination of flags, like so:

```
# Production
rose-updater.exe --url https://updates.roseonlinegame.com

# Development
rose-updater.exe --url http://rose-dev/updates trose.exe -- --init --server ROSE-DEV
```

This would create two local manifests:

- `%LocalAppData%\Rednim Games\ROSE Online\cache\updater\updates.roseonlinegame.com\local_manifest.json`
- `%LocalAppData%\Rednim Games\ROSE Online\cache\updater\ROSE-DEV\local_manifest.json`

## Tokio Console

Install tokio console `cargo install --locked tokio-console`.

To compile with support for tokio console, use the following command `RUSTFLAGS="--cfg tokio_unstable" cargo build --features console`

Now run the app and run the console to see the instrumented results `tokio-console`.
