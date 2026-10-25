# TreeTender

Reproducible workspace lifecycles for humans, agents and their tools.

TreeTender is a Rust tool with CLI, TUI and structured agent access for preparing, building, inspecting and cleaning development workspaces. Repository-local declarations describe resources; a central registry records identity, ownership, references and partial operations.

Git remains responsible for repositories and worktrees. External adapters handle database namespaces, storage and service supervision. Editors, agents and tools such as Orca invoke the same commands.

## Use

```sh
cargo build --release --locked
target/release/treetender --repo /path/to/checkout init
target/release/treetender --repo /path/to/checkout inspect
target/release/treetender --repo /path/to/checkout register
target/release/treetender --repo /path/to/checkout prepare
target/release/treetender --repo /path/to/checkout prepare --apply
```

Lifecycle commands show a plan until `--apply` is supplied. Use `plan clean --out cleanup.json` and `apply cleanup.json` to review separately. `stop --apply` retains data and checkout; `clean --keep database --apply` retains the named resource regardless of backend.

```sh
treetender tui
treetender --json agent schema
treetender --json agent resources
treetender --json status
```

See [usage](docs/usage.md), the [adapter contract](docs/adapters.md) and [examples](examples/). Put machine-specific declarations in ignored `.treetender.local.toml`. Existing live workspaces are never automatically adopted or migrated.

## Safeguards

- Primary checkout resources are protected by identity, including after a branch rename.
- Shared and borrowed references never grant destruction rights; other registered users of a resource protect it too.
- Cleanup requires an exact recorded instance and provider ownership proof. Unknown state blocks cleanup.
- Saved plans are revalidated under a process lock. Partial effects retain provisional claims and recovery history.
- Generated files are explicit, edits are preserved, and symlink escapes are refused.
- Build/check commands are separate from service startup; stop retains resource and port claims.

Adapters and tasks are trusted executable code, with the user's normal permissions. Provider correctness is part of the ownership boundary; TreeTender cannot sandbox a script that ignores its contract.

## Integrations and scope

The initial implementation includes explicit file copy/link operations, generic JSON command adapters, dependency ordering, build/check artifacts and a read-only Portman service observer. Managed service effects use adapters with real instance/ownership proof. The Portman observer does not synchronize config, start/stop services or adopt registrations.

The namespace example models database and storage lanes in disposable files. Production database/storage providers must implement the documented contract; the fixture is not a database driver. Build artifacts are observed and retained; destructive artifact cleanup requires an explicitly managed resource.

The repository is private. No public release or license has been selected. [Product direction](docs/brief.md), [implementation contract](docs/plans/2026-10-25-implementation.md) and [research](docs/research.md) explain the design.

## Build and verify

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
cargo package --locked --allow-dirty
```

Tests use temporary Git repositories and disposable provider fixtures. They require Git and Python 3; runtime does not require Python. macOS and Linux are the initial supported hosts.

## Boundaries

TreeTender reuses Git and existing service tooling. It does not replace a supervisor, secret manager, editor or agent orchestrator. Checkout creation/removal stays with Git callers and existing worktree tools; resource cleanup never implicitly removes a checkout.
