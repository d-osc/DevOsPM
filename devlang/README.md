# dev — DevLang build

This directory is the DevLang (`.dev`) implementation of the `dev` CLI. It is
a behavior-compatible port of the Rust implementation in `../src`, reads and
writes the same package store, and can be mixed freely with the Rust binary
(the registry, snapshots, shims and Dev OS extension layout are identical —
verified by cross-installing in both directions).

## Running

Requires the DevLang runtime `d` ≥ 0.5.5 on PATH (https://github.com/d-osc/DevLang)
plus the standard tools `sh`, `tar`, `sha256sum`, `chmod`, `rm`, `cp`, `find`,
`realpath` and `curl` (every Linux with coreutils + tar has these; `ar` from
binutils is additionally needed for `.deb` installs).

```sh
d main.dev -- <command> [options]     # direct
./dev <command> [options]             # through the launcher script
sh install.sh                         # install to ~/.local (dev on PATH)
```

All subcommands of the Rust build work: `install` (aliases `add`, `i`),
`list` (`--json`), `info`, `verify`, `remove` (aliases `uninstall`, `un`,
`rm`) with `--purge` for extensions, `rollback`, `run`, `pack`, `update` and
`self-update`. Global options `--root`, `--devos-data` and `--applications`
behave the same, as do the `DEV_ROOT`, `XDG_DATA_HOME`, `DEV_BIN_DIR` and
`DEV_UPDATE_API` environment variables.

`self-update` in this build swaps the **source tree** under
`$XDG_DATA_HOME/devos/dev-cli/current` (or `$DEV_CLI_ROOT`) with the
`dev-cli.tar.gz` asset of the latest d-osc/DevOsPM release, after running the
staged tree's `--version` — the same verify-then-swap flow as the Rust build.

## Tests

```sh
d test_modules.dev                 # unit checks for util/archive/manifest
sh smoke_devlang.sh                # end-to-end: pack→install→run→…→remove
```

## Deliberate differences from the Rust build

The runtime libraries of DevLang cap in-memory data at 8 MiB and offer no
flock/chmod/symlink APIs, so the port shells out to coreutils the same way
the Dev OS distro scripts do, and keeps the security checks in DevLang:

- **Extraction**: every `tar -tv` member name is validated (`safe_path`,
  duplicates, case conflicts, types, size/count limits) *before* `tar -x`
  runs, and the extracted tree is cross-checked against the validated
  listing afterwards. Sizes are enforced from the listing, hashes with
  batched `sha256sum`.
- **Hashing** happens after a download lands instead of while streaming;
  nothing is installed or executed before the digest matches.
- **Store lock**: flock is not available, so write operations take a
  `.dev-lock` marker file carrying the pid (a stale marker whose process is
  gone is reclaimed). Reads take no lock; registry writes are still atomic
  rename-over, so readers never observe torn state.
- **`dev run`** captures the child's stdout/stderr and replays them when it
  exits (the runtime pipes child stdio; there is no inherit mode). The exit
  status is exact, GUI apps are unaffected, but interactive CLI programs do
  not stream.
- **Progress lines** print plain lines on stderr like the Rust build's
  non-terminal mode; there is no animated spinner.
- **`.deb`** support needs `ar` (binutils); **`.rpm`** is not supported yet
  in this build (the Rust build converts cpio payloads internally).
- **fsync** durability calls (`sync_all`/`sync_tree`) have no DevLang API
  equivalent; committed state is still rename-atomic.
- **contextmenu args** additionally reject all control characters, not only
  NUL (stricter than the Rust build).

## Layout

| File | Rust counterpart |
| --- | --- |
| `main.dev` | `src/main.rs` (CLI parsing and dispatch) |
| `util.dev` | shared helpers (errors, shell access, ids, semver, `safe_path`) |
| `archive.dev` | hardened tar extraction + `.deb` unwrap (`manager.rs`, `native.rs`) |
| `manifest.dev` | `package.json` manifest + release templates (`manager.rs`, `metadata.rs`) |
| `store.dev` | store, registry, install/remove/rollback/verify/run/pack (`manager.rs`) + extension flows (`devos.rs`) |
| `devosx.dev` | Dev OS extension manifest rules and store layout (`devos.rs`) |
| `desktop.dev` | XDG `.desktop` entries (`desktop.rs`) |
| `shim.dev` | PATH command shims (`shim.rs`) |
| `download.dev` | HTTP(S) downloads (`download.rs`) |
| `update.dev` | GitHub releases update + self-update (`update.rs`) |
| `progress.dev` | stage/notice output (`progress.rs`) |
