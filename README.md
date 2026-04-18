# dispatch-courier-seren-cloud

A [Dispatch](https://github.com/serenorg/dispatch) courier plugin for remote parcel execution on [Seren Cloud](https://seren.ai).

This plugin implements the existing Dispatch JSONL courier plugin protocol and translates parcel/session/run requests into Seren Cloud API calls. It is intended to prove that remote execution backends can fit the current courier plugin contract without new Dispatch core protocol work.

## Discover

Dispatch 0.3.0 and later support catalog-based extension discovery. Register this repository as a catalog source once, then search and inspect it through `dispatch extension`:

```bash
dispatch extension catalog add \
  https://raw.githubusercontent.com/serenorg/dispatch-courier-seren-cloud/main/catalog/extensions.json
dispatch extension catalog refresh
dispatch extension search --kind courier seren
dispatch extension show seren-cloud
```

The catalog entry ships at `catalog/extensions.json` in this repository. `dispatch extension show` prints the install hint and source metadata.

## Install

Install the published binary directly from the catalog:

```bash
dispatch extension install seren-cloud
```

## Build From Source

Build the binary locally and install it as a Dispatch courier plugin:

```bash
cargo build --release
dispatch courier install courier-plugin.json
```

Notes:

- `courier-plugin.json` points to `./target/release/dispatch-courier-seren-cloud`, so install it from the repository root after building.
- On Windows, update `exec.command` to `./target/release/dispatch-courier-seren-cloud.exe` before installing.

## Configuration

The plugin requires:

- `SEREN_API_KEY` - your Seren Cloud API key, exported in the environment of the `dispatch` process

Optional:

- `SEREN_API_BASE` - override the API base URL (default: `https://api.serendb.com`)

## Usage

```bash
# Run a parcel through Seren Cloud
dispatch run examples/parcels/basic --courier seren-cloud --chat "hello"

# List installed couriers
dispatch courier ls

# Inspect the courier
dispatch courier inspect seren-cloud
```

## How it works

1. `dispatch run --courier seren-cloud` launches this plugin as a subprocess
2. Dispatch sends JSONL requests over stdin (`validate_parcel`, `inspect`, `open_session`, `resume_session`, `run`, `shutdown`)
3. The plugin uses the built parcel locally for:
  - courier reference validation
  - prompt resolution
  - local tool inspection
4. The plugin uses the Seren Cloud API for:
  - remote deployment/session establishment
  - remote chat/job/heartbeat runs
  - best-effort shutdown
5. Responses are returned as JSONL on stdout and Dispatch persists resume state in `backend_state`

The parcel artifact stays immutable and portable. Seren Cloud is just one possible execution target.

## Current scope and limitations

- `validate_parcel` currently validates the parcel as a generic Dispatch `custom` courier target. It does not add Seren-specific parcel constraints.
- `invoke_tool` is not implemented. The plugin advertises `supports_local_tools = false`, so Dispatch will not route direct tool execution through it.
- The plugin currently sends parcel digest, manifest JSON, and source parcel directory metadata to the Seren API. If Seren Cloud needs uploaded parcel contents or staged artifacts, that upload flow should be added inside the plugin without changing the Dispatch courier protocol.

## Protocol

This plugin implements Dispatch courier plugin protocol version 1. See [docs/extensions.md](https://github.com/serenorg/dispatch/blob/main/docs/extensions.md) for the current extension and manifest documentation.

## License

MIT
