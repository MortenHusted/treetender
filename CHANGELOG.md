# Changelog

## Unreleased

- File resources gain `mode = "template"`, which renders workspace placeholders and port leases into a private copy, link to directories as well as files, and take `optional = true` to skip an absent source.

- Lease named host ports to each workspace from a declared `[ports.NAME] range`. Leases are stable per lane, skip ports held by other workspaces or occupied by listeners, survive stop and clean, and are released by removing the declaration or `forget --only port:NAME`. Use them as `{port:NAME}` and `TREETENDER_PORT_NAME`.

- Linked checkouts inherit the primary checkout's private `.treetender.local.toml`, between their own two manifests. Declarations and tasks take `scope = "linked" | "primary"` to apply to one side only.
- Workspaces get a name at first registration (`register --name`, or derived from the directory), unique per repository and fixed afterwards. Targets accept `{workspace_name}` and `{workspace_ident}`.
- Tasks and adapter commands receive `TREETENDER_WORKSPACE_*` and `TREETENDER_REPOSITORY_ID` environment variables; `treetender env` prints them as dotenv lines.

- Add owned directory resources for per-lane build and dependency output, proven by an ownership marker and removed descriptor-relatively on clean. A reviewed `adopt --only RESOURCE` claims existing ignored, untracked output; `status --usage` reports allocated bytes.

- Add adapter-free passive resource declarations with purpose/manager notes, CLI/TUI accounting and agent discovery. Passive references never allocate, observe or mutate backend resources and remain protective during cleanup.

- Initial Rust CLI, TUI and structured agent interface.
- Central registry for Git identities, resource ownership, protected references and operation history.
- Explicit file copy/link resources and versioned JSON adapters for user-provided backends.
- Prepare, build, check, start and stop lifecycles, with declared artifact freshness and retain-by-default stop behavior.
- Reviewed cleanup and non-destructive forget, with primary/shared safeguards and interrupted-effect recovery.
- Read-only Portman service observation and disposable database/storage namespace examples.
- MIT license, public contribution and security guidance, and Linux/macOS CI.
