# Working with TreeTender

TreeTender keeps a local registry of Git workspaces, their resource ownership and lifecycle operations. Git owns checkout metadata. External tools own supervision and backend operations.

## Register existing relationships

Start by declaring which resources the lane uses and calling `register`. It records associations without executing adapter commands, creating databases or adopting existing objects. Use shared/borrowed declarations when another tool manages the lifecycle. `list` and `status` expose the recorded relationships; live observations are separate evidence and may be unavailable.

For accounting only, omit `adapter` and provide `id`, `kind` and `target`. Optional `description` records purpose and `manager` identifies the responsible tool/person. `target` supports stable workspace/repository placeholders; ports on passive declarations are informational. Use [the inventory example](../examples/inventory.toml) as a starting point. `register` updates the snapshot; it never invokes a passive resource. CLI/TUI label these entries passive and JSON lists them in `declarations`, without allocation receipts or observations.

Passive resources are excluded from prepare/start/stop/cleanup regardless of ownership mode. Passive-only dependency graphs are accounting relationships; operations requiring an actually ready prerequisite need an observer and block without one. A retained passive dependent can block deletion of its parent. Update live associations by editing the manifest and registering again; registered associations survive checkout disappearance until explicitly forgotten. Existing managed receipts survive removing an adapter and require deliberate reconciliation. External receipt import remains unavailable.

## Lane names, private overlays and lane values

Each workspace gets a name at its first registration, derived from the checkout directory (`feature/Login` becomes `feature-login`) or chosen with `register --name NAME`. Names are unique within a repository and fixed afterwards, so `{workspace_name}` and `{workspace_ident}` targets survive moves.

Private declarations for a repository whose team does not track `treetender.toml` go in the primary checkout's ignored `.treetender.local.toml`. Linked checkouts inherit it between their own `treetender.toml` and `.treetender.local.toml`; a later file replaces an earlier declaration with the same ID. Use `scope = "linked"` for declarations only lanes should get, such as an isolated database, and `scope = "primary"` for the primary's own:

```toml
# primary checkout: .treetender.local.toml
version = 1
[[resources]]
id = "cargo-target"
kind = "file"
scope = "linked"
[resources.adapter]
type = "directory"
path = "target"
```

### Port leases

Declare named ports and the range to lease them from:

```toml
[ports.web]
range = [3900, 4099]
```

`register` leases each name to the workspace: the same lane starts from the same position in the range, skips ports that another workspace holds or a local listener occupies, and keeps its lease across restarts, stop and clean. Two lanes never share a leased port, even while one is stopped. A lease moves only when its range changes, and is released when its name leaves the declarations or by `forget --only port:web`. Use the lease as `{port:web}` in targets and as `TREETENDER_PORT_WEB` in commands.

Tasks and adapter commands receive the lane values as `TREETENDER_*` environment variables. `treetender env` prints them as dotenv lines for direnv, mise or a process manager's env file; `--json env` returns an object.

## Review and execute

```sh
treetender --repo . init
treetender --repo . inspect
treetender --repo . register
treetender --repo . prepare
treetender --repo . prepare --apply
treetender --repo . build --apply
treetender --repo . check --apply
treetender --repo . start --apply
treetender --repo . stop --apply
treetender --repo . clean --keep database
```

Lifecycle commands produce a plan until `--apply` is supplied. Preparation never starts services. Stop retains persistent data and the checkout. Cleanup is explicit and can retain any named resource, including a database, storage prefix, volume or file. Primary, shared and borrowed resources are excluded from destruction. `clean --apply`, `forget --apply`, `adopt --apply` and `unregister --apply` require `--expect-fingerprint FINGERPRINT` from an earlier reviewed plan; applying a saved plan is equivalent.

For a separately reviewed operation, save and apply the exact plan:

```sh
treetender plan clean --only scratch-storage --out cleanup.json
treetender apply cleanup.json
```

Changed manifests, ownership or registry state can invalidate a saved plan. Generate a new plan and review its effects. Never edit a plan to bypass a blocker.

Unrelated operation history does not invalidate a plan. Conflicting claims, ownership, relevant references, executable resolution or material inputs do. A busy registry returns `registry_busy` after a bounded wait; retry after the other operation completes.

## Retain data while releasing ownership

When a generated file is edited, a provider reports a recorded resource missing, or an uncertain instance cannot be reconciled, explicitly forget its claim without deleting the instance:

```sh
treetender plan forget --only storage --out forget.json
# Review the released claims/ports and retained instance.
treetender apply forget.json
```

Forget requires an explicit resource selection, preserves the complete receipt in operation history, and does not grant ownership of an existing object to a later prepare. Primary workspace records remain protected. For an absent/replaced linked checkout, select its stable ID with `--repo ID`; forget can also release its stale borrowed references. Live references are changed in the declaration and refreshed through register.

## Unregister a removed checkout

A removed linked checkout keeps its record, and with it its name, port leases and registered references. A new checkout at the same path gets a new identity, so it would register as `NAME-2`. Once the checkout is gone, drop the old record by its ID:

```sh
treetender --repo WORKSPACE_ID unregister
treetender --repo WORKSPACE_ID unregister --apply --expect-fingerprint FINGERPRINT
```

The plan releases the workspace's port leases and references, then drops its record; a checkout registered later can take the name. It is blocked while the checkout still exists or its location is unknown, for the primary workspace, and while the workspace still owns a claim: clean or forget those first, since unregister never deletes or releases an allocation. Operation history stays readable with `treetender --repo WORKSPACE_ID history`.

## Adopt existing build output

Directory resources own per-lane build or dependency trees. A directory created before its declaration, or recreated by its tool, carries no ownership marker, and prepare refuses to claim it. Adopt it explicitly after review:

```sh
treetender plan adopt --only cargo-target --out adopt.json
treetender apply adopt.json
treetender status --usage
treetender plan clean --only cargo-target --out clean.json
treetender apply clean.json
```

Adopt only accepts an ignored, untracked real directory in a linked workspace, or a [PostgreSQL database](adapters.md#postgresql-databases) whose name contains the workspace's `{workspace_ident}`. `adopt --apply` requires `--expect-fingerprint`, like clean and forget. See [owned directories](adapters.md#owned-directories).

## Builds and checks

Task outcomes record their declared input fingerprints, task definition and output artifacts. Status reports fresh, stale, failed, interrupted or never-built outcomes. Freshness covers declared inputs and outputs; declare dependency artifacts as inputs to checks when they should invalidate prior results. It is not a guarantee about undeclared environment variables or external mutable data. Builds always run when explicitly applied; TreeTender does not silently skip them based on freshness.

## Central discovery

`list` shows registered workspaces. `status` shows recorded resources and read-only provider observations; `history` exposes operation outcomes and incomplete work. Provider observations execute declared inspection commands. Use `--state-dir` or `TREETENDER_STATE_DIR` to choose a separate registry. On macOS the default lives below `~/Library/Application Support/treetender`; on Linux below `$XDG_STATE_HOME/treetender` or `~/.local/state/treetender`.

Register explicitly before applying operations. Registration records identity without adopting existing databases or starting services. Primary protection follows Git checkout identity rather than the branch named `main`. Sharing a resource gives a worktree permission to use it, never to destroy it.

## Agents and other callers

```sh
treetender --json agent schema
treetender --json agent resources
treetender --json agent read treetender://adapter
treetender --json plan prepare
```

The CLI emits a versioned JSON envelope. Saved plans contain the core plan directly, so they can be passed back to `apply` without scraping output. Command errors return a nonzero status. A partially failed apply retains completed receipts and returns a nonzero status; inspect its result and history before retrying.

The TUI uses the same core for plans and execution. Open it with `treetender tui`; use `[` and `]` to switch registered workspaces, arrow keys to select a resource, and space to retain it. Keys `1`–`6` generate lifecycle plans; `7` plans forgetting the selected resource, `8` plans adopting it and `9` plans unregistering a removed workspace. `a` applies the reviewed plan, and Escape cancels it. Agent instruction files, skills and tool configuration are explicit file resources, without requiring any particular agent provider.

## Configuration and trust

Portable configuration lives in `treetender.toml`. Machine-local overrides live in `.treetender.local.toml` and should be ignored by Git. Keep credentials in existing credential tooling; command declarations and registry records are not a secret store. Start with explicit resource lists rather than copying every ignored file.

External adapters and build commands are trusted executable code. TreeTender checks declarations, receipts and ownership, but arbitrary scripts retain their normal operating-system permissions. Review repository commands before applying a plan from an unfamiliar project. Inspection through an adapter can execute that adapter's inspection command; the built-in Git/config inspection does not provision resources.

No existing live workspace is automatically migrated. Use an isolated state directory and disposable fixtures while evaluating the initial release.
