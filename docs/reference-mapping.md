# Reference implementations mapped to TreeTender

Inspected: 2026-10-25. This document records source observations and candidate design mappings, not an approved implementation plan. The checkouts were cloned for inspection only; no reference build, setup command, hook, test or service was run. No external source code was copied into TreeTender.

## Local reference inventory

Each project is a separate canonical checkout under the central GitHub checkout root, using its actual origin owner and repository name. All four checkouts were clean after inspection.

| Project / local namespace | Inspected commit | Declared license |
|---|---|---|
| `max-sixty/worktrunk` | `a6c26c787e7e30d989f04e7012029bfd55ad6ff1` | [MIT OR Apache-2.0](https://github.com/max-sixty/worktrunk/blob/a6c26c787e7e30d989f04e7012029bfd55ad6ff1/LICENSE#L1) |
| `coderabbitai/git-worktree-runner` | `d576398fc0642dc91cc248f63635e4dea283d416` | [Apache-2.0](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/LICENSE.txt#L1) |
| `cachix/devenv` | `80591248bdcd9c12d7d0e8328a5f12972246626f` | [Apache-2.0](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/LICENSE#L1) |
| `HanifCarroll/devtree` | `2eed8a53c9a96514abdd4f3d45d8354fadc61c0e` | [MIT](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/LICENSE#L1) |

## What the code establishes

### Worktrunk: freeze selected work and preserve execution context

Its hook planner selects commands once, retaining source and worktree anchor in a plan consumed by executors. Execution does not re-read configuration to choose a different command set after Git has changed the checkout. This is a useful pattern for a TreeTender plan that must still refer to the work the caller inspected. It is not evidence that arbitrary hook effects are known or reversible. [Plan invariant](https://github.com/max-sixty/worktrunk/blob/a6c26c787e7e30d989f04e7012029bfd55ad6ff1/src/commands/hook_plan.rs#L1-L40), [executor interface](https://github.com/max-sixty/worktrunk/blob/a6c26c787e7e30d989f04e7012029bfd55ad6ff1/src/commands/hook_plan.rs#L343-L373).

Worktree-targeted children scrub inherited Git discovery variables, rather than assuming changing cwd is sufficient. TreeTender should apply that lesson when invoking Git or scripts in another workspace; caller-specific environment can otherwise point an operation at the wrong index or checkout. [Execution-context rules](https://github.com/max-sixty/worktrunk/blob/a6c26c787e7e30d989f04e7012029bfd55ad6ff1/src/shell_exec.rs#L687-L750).

Ignored-file copying supports dry-run and structured output, with a require-include gate. Without that gate or an include file it can select all ignored files. For TreeTender, study selection/reporting but retain explicit declarations for private files and mutable agent state; do not adopt broad copying by default. [Copy selection and gate](https://github.com/max-sixty/worktrunk/blob/a6c26c787e7e30d989f04e7012029bfd55ad6ff1/src/commands/step/copy_ignored.rs#L40-L153).

### Git Worktree Runner: hooks are practical, but effect ownership is extra

Creation reports a stable escaped record stream with path, branch and hook status; hook failures prevent the success records. This is a useful example of distinguishing checkout creation from completed preparation. Human logs and executable hook output are kept separate from the machine records. [Record output](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/commands/create.sh#L88-L103), [creation completion](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/commands/create.sh#L261-L285).

Pre-remove hooks run while the directory still exists and normally block deletion on failure; post-remove hooks can only warn after deletion. Its force flag can continue past a failed pre-remove hook. TreeTender should run required resource cleanup before removing the checkout, and distinguish deleting a checkout from accepting unresolved external resources rather than borrowing force semantics unchanged. [Remove order and failures](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/commands/remove.sh#L38-L85).

Hooks receive repository/worktree/branch context and run as commands. Its adapter registry also demonstrates simple declared tools with override files and a PATH fallback, but those are editor/agent launch adapters, not a generic database/storage resource protocol. [Hook execution](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/hooks.sh#L265-L332), [adapter registry](https://github.com/coderabbitai/git-worktree-runner/blob/d576398fc0642dc91cc248f63635e4dea283d416/lib/adapters.sh#L1-L60).

### devenv: capabilities, task outcomes and common activity

The [devenv source study](reference-devenv.md) supplies pinned evidence for these candidate mappings:

- Distinguish a successful finite task from a ready long-running process; dependency completion is not always dependency success.
- Declare supported outcomes separately from the transport used to achieve them; unknown optional capabilities default to unsupported.
- Produce typed activity once for CLI/TUI/structured consumers, while keeping that event stream separate from durable resource ownership.
- Treat allocation-time socket reservations, provider assignments and long-lived stopped-workspace claims as different concerns.
- Keep product build/test/package proof separate from managed-workspace task/resource proof.

Its inspected persistent PostgreSQL state and environment/store GC do not establish a generic persistent-resource destroy contract. Do not import Nix evaluation, a process supervisor or the large crate layout merely to obtain the useful patterns above.

### Rust DevTree: a compact vertical slice and useful limits

The source has explicit file-copy lists, leaves existing destinations unchanged, runs ordered setup commands and supports a JSON status result. This is a useful small example of making a fresh worktree usable without mixing in issue/PR orchestration. Its app/URL configuration uses a fixed provider enum, however; that is not the open resource interface TreeTender needs. [Configuration types](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/src/main.rs#L115-L173), [copy/setup behavior](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/src/main.rs#L394-L450).

Repository state is keyed from the canonical Git common-directory path, and cleanup checks app/dirty state before asking Git to remove the worktree. TreeTender can learn from the small operation flow but must solve relocation identity and separately owned persistent resources rather than inherit those semantics unchanged. [State loading](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/src/main.rs#L314-L345), [path key](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/src/main.rs#L800-L823), [cleanup](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/src/main.rs#L603-L641).

The compact CI checks formatting, locked compilation, lint and tests. This is a starting reference for product build proof, not evidence of TreeTender's eventual supported package targets. [CI](https://github.com/HanifCarroll/devtree/blob/2eed8a53c9a96514abdd4f3d45d8354fadc61c0e/.github/workflows/ci.yml#L1-L24).

## Mapping patterns onto the lifecycle

| TreeTender concern | Candidate pattern | Remaining TreeTender responsibility |
|---|---|---|
| Inspect and plan | Worktrunk's selected command plan; provider capability declarations | Resolve current identity/ownership, expose opaque effects and keep planning free of setup mutations |
| Prepare files/context | Explicit copying and source/workspace context | Provenance, edited-file handling, private overlays and mutable agent-state isolation |
| Prepare persistent resources | Provider-specific task execution and outputs | Own exact resource handles across databases, storage and custom backends; task success alone does not prove ownership |
| Build/check | Finite tasks with success/output evidence | Define declared artifacts and freshness separately from running-service readiness |
| Start/stop | Capability-aware service adapters | Delegate to Portman or another owner and preserve stopped/retained intent |
| Clean | Required cleanup before checkout removal | Generic resource selection, retention and dependency ordering; never equate data cleanup with database cleanup alone |
| Retry and recover | Completed-step results plus observed provider state | Reconcile partial creation and interruption without inventing universal rollback |
| CLI/TUI/agents | One typed activity/result model with several consumers | Equivalent operations/policies and machine contracts independent of terminal rendering |
| Remove/relocate | Git discovery and ordered checkout operations | Keep retained resource provenance and stable identity when paths disappear or change |

## The resource abstraction to test next

The user's correction establishes that persistent data is not a database-specific concern. The following is a candidate contract to exercise, not a schema or Rust type definition:

- A resource has a workspace owner, an adapter/provider, an exact external target/handle and relevant dependencies.
- Its retention intent and observed state are distinct; inability to inspect is not evidence that it is absent.
- Its adapter declares what inspection, preparation, activation/deactivation and removal it can actually perform. Unsupported operations remain explicit.
- A plan identifies the selected effect and target; execution records the actual result/handle so subsequent inspect, retry or cleanup can find it.
- An arbitrary script can remain an opaque task. To participate as a managed resource it must supply the observations, handles and cleanup behavior the common contract requires; success exit status alone is insufficient.

Exercise the same contract against a database namespace, a storage prefix within a shared bucket, a named volume and a custom scripted resource. Keep the shared backing server/bucket/volume host outside workspace ownership. Do not introduce central resource-specific keep flags as the abstraction; the same selection/retention intent must work through each adapter.

Before accepting the contract, walk through two important cases: one selected resource is retained while its dependant is cleaned, and one resource is created before a later step fails. Add relocation and checkout-removed cases so the registry cannot rely solely on scripts or handles stored inside the disappearing checkout.

These fixtures are for interface design. They do not authorize provisioning live databases/storage, running the cloned tools or migrating existing workspaces. Stop-retains remains settled; exact explicit-clean defaults and the integration protocol remain open.
