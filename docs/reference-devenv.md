# devenv: lifecycle and interface reference

Inspected: 2026-10-25. Checkout: `~/gh/cachix/devenv`.

Commit: `80591248bdcd9c12d7d0e8328a5f12972246626f`. License: Apache-2.0, identified in [LICENSE](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/LICENSE#L1-L5).

This is source inspection only. No devenv/Nix command, task, test, service or hook was executed. The reference checkout remains unchanged. Observed behaviors below are distinct from candidate mappings for TreeTender.

## 1. Dependencies express the condition being waited for

**Observed:** Task types distinguish one-shot actions from long-running processes. Dependency conditions distinguish started, ready, succeeded and completed; completed can allow a dependent to proceed despite a terminal failure. [Task and dependency types](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/types.rs#L7-L58).

The scheduler rejects graph cycles. Its dependency wait loop distinguishes satisfied, not-yet-satisfied and never-satisfiable dependencies, and gives cancellation precedence over failure. [Cycle handling](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/tasks.rs#L656-L667), [dependency waiting](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/tasks.rs#L1337-L1393).

**Candidate mapping:** TreeTender can distinguish a completed file-copy/build task from a ready external service. A migration step succeeding and a process merely starting should not satisfy the same condition. Do not import the full scheduler or its dependency vocabulary before simple file/data/service examples establish what TreeTender needs.

## 2. Task outputs and cache checks are separate from resource ownership

**Observed:** Task configuration can declare a command, status probe, file-modification inputs, structured input, working directory and environment. It also distinguishes ordering edges from task selection through `wanted_by`. [Task configuration](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/config.rs#L7-L39).

A successful status probe can reuse cached outputs. A nonzero probe exit is treated as a cache miss, while failure to execute the probe is a task failure. File-state cache entries are updated only on successful task execution; task outputs are read from a JSON file. [Probe handling](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/task_state.rs#L500-L575), [success/failure recording](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-tasks/src/task_state.rs#L641-L709).

**Candidate mapping:** Preserve observed outputs and completed-step results for inspect/retry, but keep them separate from persistent resource handles and ownership. A cache miss is not proof that a database, storage prefix or volume is absent. The reviewed task cache does not establish generic rollback or safe replay of arbitrary side effects.

## 3. Capability negotiation is separate from transport details

**Observed:** Process managers advertise optional background-start, attach, readiness-wait, individual-control and cold-start-subset capabilities. A separate adapter declaration describes terminal requirements, stop mechanism and client protocol. Unknown capabilities default false; unknown managers receive no optional operations. Unsupported operations return explicit errors. [Capability and adapter types](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-processes/src/manager_capabilities.rs#L31-L169).

The CLI evaluates capability and adapter declarations separately and checks background/subset requirements before the corresponding launch. [Declaration loading and checks](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv/src/devenv/mod.rs#L2820-L2893).

**Candidate mapping:** A TreeTender integration should explicitly state what can be inspected, prepared, activated, stopped or destroyed. Database/storage adapters need not pretend to be services. Capability answers describe supported outcomes; command/IPC details describe how the adapter achieves them. This is a useful pattern for a Portman adapter and user-supplied tools without requiring a new supervisor or Nix evaluation.

## 4. Port reservations protect the allocation window, not every stopped workspace

**Observed:** Ports are cached by process/port name. The allocator stores bound listeners and can snapshot allocations for cache replay; existing manager assignments can seed the allocator without binding. A reservation guard releases its listeners when dropped. [Reservations and allocation records](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-core/src/ports.rs#L43-L128), [manager seeding](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-core/src/ports.rs#L168-L200), [allocation entry point](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-core/src/ports.rs#L240-L268).

**Candidate mapping:** Use a real reservation mechanism to close allocation races, and reconcile provider assignments instead of inventing conflicting values. This source does not demonstrate the durable cross-workspace lease policy TreeTender needs for intentionally retained but stopped workspaces. Reservation handoff and long-term ownership remain separate design questions.

## 5. Service state persists independently of starting the process

**Observed:** The PostgreSQL module puts `PGDATA` under `DEVENV_STATE/postgres`. Startup initializes only a missing data directory, reuses existing data, and invokes the server after its setup. [Initialization and startup](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/src/modules/services/postgres.nix#L150-L197), [data path](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/src/modules/services/postgres.nix#L451-L474).

The inspected garbage-collection entry point scans environment-history links and delegates store collection to the backend; this does not establish a generic database/storage/volume destroy contract. [GC entry point](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv/src/devenv/gc.rs#L10-L29).

**Candidate mapping:** Retain state separately from process activity, but model data as provider-owned resources rather than central database special cases. TreeTender still needs explicit retention, inspectable destroy effects and recorded handles for databases, object-storage prefixes, volumes and custom state. Do not turn environment-cache garbage collection into workspace-data cleanup by analogy.

## 6. CLI, TUI and structured activity can share execution evidence

**Observed:** A typed, serializable activity stream covers builds, tasks, commands, processes and operations. The run selects TUI or console renderers over the same activity-event type; tracing-only mode does not need a renderer. A separate layer emits activity events as JSONL. [Activity events](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv-activity/src/events.rs#L9-L24), [renderer selection](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv/src/main.rs#L728-L805), [JSONL export](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv/src/tracing/activity_json_layer.rs#L13-L55).

Its MCP process tools translate structured requests into the process protocol, including list/status/start/stop. [Process tools](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/devenv/src/mcp.rs#L389-L467).

**Candidate mapping:** Produce operation/resource events once and render them for CLI, TUI and agents. Machine results should not be inferred from formatted terminal text. Activity/telemetry alone is not TreeTender's durable resource-ownership journal, and MCP is an optional caller adapter rather than a requirement to operate the product.

## 7. Product builds and managed-workspace tasks are separately testable

**Observed:** The Cargo workspace separates core, task, process, activity, TUI, shell and integration-test crates; package defaults intentionally include user-facing crates. The test workflow builds the tool before exercising its own environment checks and fixture test runner. [Cargo membership/defaults](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/Cargo.toml#L1-L39), [build-and-test workflow](https://github.com/cachix/devenv/blob/80591248bdcd9c12d7d0e8328a5f12972246626f/.github/workflows/test.yml#L59-L86).

**Candidate mapping:** Include product build/test/package proof and fixture workspace lifecycle proof from the first runnable TreeTender slice. Keep UI/agent consumers on the same core behavior. The large crate layout and Nix build machinery are scope-specific examples, not a reason to begin TreeTender with a comparable architecture.

## Concrete gaps to resolve in TreeTender

- Generic persistent-data cleanup: identify actual owned resources, retained dependencies and destroy effects across providers; no database-specific central lifecycle or global `--keep-db` abstraction is implied by this reference.
- Stable checkout/resource identity through relocation and explicit adoption; inspected paths/task cache are not that contract.
- Durable reservations for stopped workspaces, separate from temporary OS socket reservation.
- Script-side effects and cleanup receipts: task output is useful evidence, but arbitrary script actions are not automatically discoverable, idempotent or reversible.
- Provider-neutral planning without running setup or depending on Nix; preserve Portman's process ownership and fail explicitly when an adapter cannot satisfy a requested operation.

These are research-derived candidate mappings, not settled implementation requirements. Relevant source was inspected in bounded ranges; no claim is made that all devenv service modules or storage backends were audited.
