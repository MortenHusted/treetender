# Working with TreeTender

TreeTender keeps a local registry of Git workspaces, their resource ownership and lifecycle operations. Git owns checkout metadata. External tools own supervision and backend operations.

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

Lifecycle commands produce a plan until `--apply` is supplied. Preparation never starts services. Stop retains persistent data and the checkout. Cleanup is explicit and can retain any named resource, including a database, storage prefix, volume or file. Primary, shared and borrowed resources are excluded from destruction. `clean --apply` and `forget --apply` require `--expect-fingerprint FINGERPRINT` from an earlier reviewed plan; applying a saved plan is equivalent.

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

The TUI uses the same core for plans and execution. Open it with `treetender tui`; use `[` and `]` to switch registered workspaces, arrow keys to select a resource, and space to retain it. Keys `1`–`6` generate lifecycle plans; `7` plans forgetting the selected resource. `a` applies the reviewed plan, and Escape cancels it. Agent instruction files, skills and tool configuration are explicit file resources, without requiring any particular agent provider.

## Configuration and trust

Portable configuration lives in `treetender.toml`. Machine-local overrides live in `.treetender.local.toml` and should be ignored by Git. Keep credentials in existing credential tooling; command declarations and registry records are not a secret store. Start with explicit resource lists rather than copying every ignored file.

External adapters and build commands are trusted executable code. TreeTender checks declarations, receipts and ownership, but arbitrary scripts retain their normal operating-system permissions. Review repository commands before applying a plan from an unfamiliar project. Inspection through an adapter can execute that adapter's inspection command; the built-in Git/config inspection does not provision resources.

No existing live workspace is automatically migrated. Use an isolated state directory and disposable fixtures while evaluating the initial release.
