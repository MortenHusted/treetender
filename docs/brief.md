# Product brief

Updated: 2026-10-25

The [implementation contract](plans/2026-10-25-implementation.md) resolves the initial command, registry and adapter choices for this agreed direction. See [usage](usage.md) and [adapter formats](adapters.md) for executable interfaces. The [lifecycle model](lifecycle.md) records the earlier design exploration.

## Problem

A Git worktree provides a source checkout, but a runnable development workspace also needs local files, isolated databases, configuration and service registrations. Existing personal tooling performs that work through centrally hard-coded application adapters and generated shims containing absolute paths.

That arrangement makes relocation expensive, hides resource ownership and gives different applications inconsistent lifecycle behavior. Arbitrary setup hooks can run commands, but a caller also needs to understand what those commands created, what remains active and what selective cleanup will do.

## Agreed direction

- Make resource registration and accounting per tree/lane the core: record identities, associations, ownership/use and retained state while existing tools can retain lifecycle responsibility. Backend mutations are optional integrations, not prerequisites for accounting.
- Build a standalone Rust product with CLI, TUI and agent access included from the beginning, sharing the same lifecycle behavior.
- Describe workspace preparation and cleanup locally to the repository, with central discovery/registration of workspaces and their resources.
- Support file copying, isolated persistent resources and optional setup/teardown operations; persistent resources can be database lanes, storage namespaces, volumes or user-defined state.
- Reuse Git's repository/worktree facilities rather than maintain a competing model of Git state.
- Treat Portman as an integration. It must not become a mandatory dependency or define the product's entire resource model.
- Allow other people to bring their own scripts and tools through a small integration contract.
- Let editors, agents and orchestrators call ordinary setup/teardown/status commands without embedding product logic of their own.
- Keep identity separate from current filesystem location. Execution uses real paths; portable declarations should not hard-code one developer's checkout paths.
- Keep personal inputs and machine overrides private. Introducing tracked defaults into an existing team repository requires an explicit opt-in.
- Retain the checkout and data by default when ending a session; make explicit cleanup straightforward.
- Express retention/cleanup generically per owned resource and delegate backend-specific effects to its adapter; the core must not assume persistent data always means a database.
- Include both TreeTender's own build/test/package workflow and managed-workspace build tasks/artifacts in the initial scope.
- Include workspace agent resources as well as the structured interface through which agents operate TreeTender.

## Responsibilities and ownership

| Concern | Intended owner |
|---|---|
| Repository history and worktree metadata | Git |
| Resource accounting, relationships and optional lifecycle coordination | TreeTender |
| Process supervision and routing | Portman or another service integration |
| Application-specific preparation and database operations | Integration commands or repository-owned tasks |
| Credentials and secret values | Existing secret tooling; declarations reference inputs without storing values |
| Agent sessions and editor behavior | The calling tool |

Central registration must distinguish a discovery index from records of allocated resources. Rebuilding an index must not silently forget a stopped workspace's database or port ownership. Forks, local-only repositories and separate clones must be accounted for when defining identity.

## Candidate first scope

Start with an explicit workspace lifecycle and a small number of integration paths, not a universal environment platform. Use disposable repositories and database/service fixtures before touching existing workspaces.

- Inspect an existing checkout or Git worktree and resolve its local declaration.
- Produce a plan describing requested effects, dependencies and destructive steps.
- Apply preparation separately from starting services.
- Treat workspace builds and checks as operations with declared inputs/outputs, separate from service startup and data cleanup.
- Report owned resources and incomplete work in structured form.
- Plan and perform selective cleanup, including keeping a database lane when requested.
- Exercise file handling, one representative database integration and the Portman boundary.
- Demonstrate a user-supplied script integration through the same contract.
- Exercise equivalent behavior through CLI, TUI and noninteractive agent access; establish the product's own build/test/package baseline with the first runnable implementation.

This scope is proposed for refinement, not a commitment to an implementation order or plugin API.

## Behaviors to establish before adoption

- No-start behavior is consistent across integrations. Inspecting or preparing a workspace does not implicitly start unrelated services.
- Teardown only acts on resources whose ownership is established, and requested preservation survives retries.
- Partial setup failures report completed effects and recovery options. Arbitrary external operations are not assumed transactional or universally reversible.
- Ports and resource names account for stopped workspaces and concurrent callers.
- Moving a checkout updates its location without treating it as an unrelated new workspace or destroying its resources.
- Local files and agent context preserve the distinction between shared inputs and mutable per-workspace state.
- Agents can discover the interface, inspect a plan, invoke it without interactive prompts and interpret stable results and errors.
- Repeated operations have defined behavior; recovery does not require guessing which commands previously succeeded.
- Ending a normal session retains data and the checkout; explicit cleanup can keep selected resources without manual database/registration discovery.
- TUI, CLI and agent callers share the same plans, effects and retention rules; build status is distinguishable from service health.

## Further adoption decisions

- How to accept externally issued allocation receipts without taking over their lifecycle. Adapter-free passive declarations are implemented; external receipt import remains open.

- Which production database/storage providers should be supplied beyond the generic contract and disposable fixtures?
- What ownership evidence should allow adopting existing live workspaces and Portman registrations?
- Which wider platforms and package distribution targets should be supported?
- Which binary/package distribution channels to support beyond the public MIT-licensed source.

## Initial exclusions

The first design does not require a new process supervisor, container runtime, credential store, remote control plane, agent scheduler or automatic merge workflow. It does not authorize migration or retirement of existing tooling. The source is MIT-licensed; no binary release has been published.
