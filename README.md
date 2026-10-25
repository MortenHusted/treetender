# TreeTender

Resource accounting and optional lifecycles for Git workspaces.

## How this was made

TreeTender's initial implementation was entirely written by AI agents, driven and directed by [Morten Husted Lochmann](https://github.com/MortenHusted). I set the direction, define the requirements and make the product decisions. Agents write the implementation, tests and documentation, and perform reviews.

That is an account of how the project was built. Judge it by the code, reproducible tests and documented behavior. TreeTender is experimental, and its adapters run with your permissions.

TreeTender records which resources belong to or are used by each Git checkout or lane: databases, storage, files, ports and services. Repository-local declarations describe those relationships; a central registry records stable identity, ownership/use, retained state and operation history. CLI, TUI and structured agent access share that accounting.

Existing tools can continue to create and operate the resources. Registering a database with a lane does not make TreeTender its database manager. Optional lifecycle integrations let TreeTender invoke existing setup/teardown commands when requested; a dedicated MySQL or PostgreSQL driver is not a prerequisite for recording an association.

Git remains responsible for repositories and worktrees. Portman or another supervisor remains responsible for processes. Editors, agents and tools such as Orca use the same registry and lifecycle commands.

## Register and account first

`register` records checkout identity and declared resource relationships without invoking adapters, creating resources or taking over their lifecycle. Use `borrowed` or `shared` declarations for resources managed elsewhere. `list` discovers workspaces; `status` distinguishes declarations and managed receipts from live observations. A recorded association alone does not prove backend existence, health or cleanup authority.

The current manifest schema still requires an adapter on each resource, including references. An observation adapter may wrap existing tooling; it needs no mutation capabilities. Accounting-only registration without any adapter, and direct import of externally issued allocation receipts, remain interface gaps. Do not interpret those gaps as a requirement to build database-management features.

## Use

```sh
cargo build --release --locked
target/release/treetender --repo /path/to/checkout init
target/release/treetender --repo /path/to/checkout inspect
target/release/treetender --repo /path/to/checkout register
target/release/treetender --repo /path/to/checkout prepare
target/release/treetender --repo /path/to/checkout prepare --apply
```

Lifecycle commands show a plan until `--apply` is supplied. Cleanup and forgetting additionally require the reviewed fingerprint, or a saved plan: use `plan clean --keep database --out cleanup.json`, review it, then `apply cleanup.json`. The named resource is retained regardless of backend. `stop --apply` retains data and checkout.

```sh
treetender tui
treetender --json agent schema
treetender --json agent resources
treetender --json status
```

See [usage](docs/usage.md), the [adapter contract](docs/adapters.md) and [examples](examples/). Put machine-specific declarations in ignored `.treetender.local.toml`. Existing live workspaces are never automatically adopted or migrated.

For bugs and questions, see [support](SUPPORT.md). Read [contributing](CONTRIBUTING.md) before proposing changes and [the security policy](SECURITY.md) before reporting a vulnerability.

## Safeguards

- Primary checkout resources are protected by identity, including after a branch rename.
- Shared and borrowed references never grant destruction rights; other registered users of a resource protect it too.
- Cleanup requires an exact recorded instance and provider ownership proof. Unknown state blocks cleanup.
- Saved plans are revalidated under a process lock. Partial effects retain provisional claims and recovery history.
- An external provider unexpectedly reporting a prepared instance missing does not erase its allocation. Explicit `forget --only RESOURCE` plans release claims while retaining instances and audit receipts.
- Generated files are explicit, edits are preserved, and symlink escapes are refused.
- Build/check commands are separate from service startup; stop retains resource and port claims.

Adapters and tasks are trusted executable code, with the user's normal permissions. Provider correctness is part of the ownership boundary; TreeTender cannot sandbox a script that ignores its contract.

## Integrations and scope

The initial implementation includes explicit file copy/link operations, generic JSON command adapters, dependency ordering, build/check artifacts and a read-only Portman service observer. Managed service effects use adapters with real instance/ownership proof. The Portman observer does not synchronize config, start/stop services or adopt registrations.

The namespace example models database and storage lanes in disposable files. It demonstrates optional managed effects and is not a database driver. Integrations that actually create or destroy resources must implement the ownership contract; associations with externally managed resources do not grant those capabilities. Build artifacts are observed and retained; destructive artifact cleanup requires an explicitly managed resource.

TreeTender is available under the [MIT License](LICENSE). There is no published binary release yet. [Product direction](docs/brief.md), [implementation contract](docs/plans/2026-10-25-implementation.md) and [research](docs/research.md) explain the design.

If you use TreeTender downstream, I would appreciate a short note in an issue or a link to what you built. This is an optional courtesy, not an additional license condition.

## Install

```sh
brew install mortenhusted/tap/treetender
treetender --version
```

The initial Homebrew formula builds a pinned source revision with locked dependencies. It installs the CLI/TUI and bundled documentation/examples; it does not start services or adopt existing workspaces. Agent callers can discover the interface with `treetender --json agent schema`.

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
