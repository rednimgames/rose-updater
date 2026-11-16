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

### Updating while the game is running

Close all ROSE Online clients before starting an update or Repair. The launcher
checks for `trose`, `trose.exe`, and the configured `--exe` name before file work,
including self-updates, forced rechecks, and `--verify`. It checks again before
modifying game files after downloading metadata or scanning local files. Matching
is case-insensitive and applies to clients in any installation directory.

A running client stops the operation with `ROSE-405`; failure to inspect running
processes stops it with `ROSE-406`. Headless mode emits an `error` event and exits
with code 1, without emitting `done`. Close the game and restart the launcher,
or retry Repair. Failed or cancelled repairs keep Play disabled until a subsequent
repair and runtime check succeed.

### DNS fallback

Manifest, archive, launcher news/image, and Visual C++ runtime downloads use
public DNS in this order: Cloudflare (`1.1.1.1`), Google (`8.8.8.8`), then Google
(`8.8.4.4`). Each server gets at most three seconds per lookup before the updater
tries the next; lookup errors and missing records also trigger fallback. Both
IPv4 and IPv6 destinations are supported, and successful answers are cached
according to their DNS TTL.

This policy applies inside the updater and does not change the computer's DNS
settings. System/ISP DNS is not used as a fallback. Local hosts-file entries and
IP-address URLs still work; private update hostnames must be listed in the hosts
file or supplied as IP addresses. All-server failures are logged with the
hostname and resolver errors. An HTTP proxy may resolve destination names itself.

### Windows Visual C++ runtime

The Windows remote `manifest.json` can declare a minimum x64 runtime:

```json
"vc_runtime_min_version": "14.44"
```

This is a top-level field alongside `version`, `updater`, and `files`. Versions
have two to four numeric components (`14.44`, `14.44.35211`, or
`14.44.35211.0`); missing build/revision components are treated as zero.

The updater reads the requirement from the server on every install, normal
update (even when no game files changed), Repair, and `--verify`, including
`--headless --verify`. Windows checks the system `msvcp140.dll` after the file
operation succeeds. If it is missing or too old, the updater downloads the
latest Microsoft v14 x64 redistributable and requests elevation to install it.
It does not download or elevate when the runtime already meets the requirement.
macOS and Linux ignore this field. Older manifests without the field skip this
step, and older updaters ignore the additional field.

The Windows archiver emits `14.44` by default. Set the requirement when producing
a Windows release (also supported when archiving on another platform):

```sh
rose-updater-archiver input output --vc-runtime-min-version 14.50
```

To raise the requirement later, change that build argument or the field in the
published Windows manifest. No new updater build or game-file hash change is
needed. Keep the requirement in sync with the client's build toolset; the
download endpoint supplies the latest **v14** runtime, not older VC++ families.
The initial rollout needs this updater release and a manifest containing the
field. The Windows setup installer's bundled redistributable is independent.

Stop cancels file verification only. Once the runtime phase begins, Repair stays
disabled until the installer exits; closing the window also waits for completion.
Installation failures and restart-required results keep Play disabled. Exit code
1638 is accepted only if a fresh DLL check meets the manifest requirement.

Headless progress adds the `installing-runtime` stage. Successful installation
adds an optional `runtime_note` to the existing `done` event; unchanged runtimes
preserve the existing event shape. A runtime failure or required restart emits
`error` and exits with code 1 instead of emitting `done`, so callers do not launch
the game before its prerequisite is ready.

### Native launcher

The launcher uses FLTK and targets Windows, macOS, and Linux, including Steam Deck.
Its news cards load directly from the ROSE website's JSON API. No WebView2,
WebKitGTK, embedded browser, or JavaScript runtime is required. News and image
requests run independently of game updates; a news outage does not prevent
updating or launching the game. Article and social links open in the default
browser.

The original launcher artwork, font metrics, two-large/three-small news grid,
and update controls are retained. Artwork and font sources are recorded in
`res/NEWS-ASSETS.txt`. Fonts are bundled and loaded privately from the app cache.

Linux builds still require FLTK's X11/Pango/Cairo libraries and Wayland development
files for native dialogs (see the packages in
`.github/workflows/ci.yaml`). Test release binaries on the intended Linux/SteamOS
runtime, including Steam Deck Desktop and Gaming modes; a CI build alone does
not validate window management, touch input, or game launch handoff.

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
