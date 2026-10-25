# Product brief

Updated: 2026-10-25

Status: agreed direction with open design questions. This is not an implementation-ready specification. Command names, configuration syntax and the integration protocol have not been chosen.

## Problem

A Git worktree provides a source checkout, but a runnable development workspace also needs local files, isolated databases, configuration and service registrations. Existing personal tooling performs that work through centrally hard-coded application adapters and generated shims containing absolute paths.

That arrangement makes relocation expensive, hides resource ownership and gives different applications inconsistent lifecycle behavior. Arbitrary setup hooks can run commands, but a caller also needs to understand what those commands created, what remains active and what selective cleanup will do.

## Agreed direction

- Build a standalone Rust CLI, designed for agents and humans from the beginning.
- Describe workspace preparation and cleanup locally to the repository, with central discovery/registration of workspaces and their resources.
- Support file copying, isolated database lanes and optional setup/teardown operations.
- Reuse Git's repository/worktree facilities rather than maintain a competing model of Git state.
- Treat Portman as an integration. It must not become a mandatory dependency or define the product's entire resource model.
- Allow other people to bring their own scripts and tools through a small integration contract.
- Let editors, agents and orchestrators call ordinary setup/teardown/status commands without embedding product logic of their own.
- Keep identity separate from current filesystem location. Execution uses real paths; portable declarations should not hard-code one developer's checkout paths.
- Keep personal inputs and machine overrides private. Introducing tracked defaults into an existing team repository requires an explicit opt-in.

## Responsibilities and ownership

| Concern | Intended owner |
|---|---|
| Repository history and worktree metadata | Git |
| Requested workspace lifecycle and resource relationships | TreeTender |
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
- Report owned resources and incomplete work in structured form.
- Plan and perform selective cleanup, including keeping a database lane when requested.
- Exercise file handling, one representative database integration and the Portman boundary.
- Demonstrate a user-supplied script integration through the same contract.

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

## Open decisions

- What is the smallest useful resource model and lifecycle vocabulary?
- How do generic script actions declare effects, report resource handles and supply cleanup behavior?
- Which integration operations are needed for planning, application, inspection and recovery?
- Where do portable declarations, private overlays and runtime records live, and how are their versions handled?
- How are repository, checkout and workspace identities represented and rediscovered after relocation?
- Which tool owns port reservation, and how are existing registrations reconciled?
- How should existing workspaces be adopted without starting services or replacing their data?
- Which parts of existing Rust tooling are worth reusing, and which need independent implementation?

## Initial exclusions

The first design does not require a new process supervisor, container runtime, credential store, remote control plane, agent scheduler or automatic merge workflow. It does not authorize migration or retirement of existing tooling. No public release or open-source license has been selected.
