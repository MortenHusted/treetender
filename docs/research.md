# Research and starting evidence

Updated: 2026-10-25

This is context for design decisions, not a dependency selection or endorsement of untested behavior.

The [local reference mapping](reference-mapping.md) pins the inspected source commits and maps their lifecycle patterns onto TreeTender. The [devenv study](reference-devenv.md) traces its tasks, process capabilities, activity and persistent-state handling in more detail.

## Lessons from existing tooling

Inspection of the existing personal `worktree-setup` at commit `3bf71ea45fbc6a544e449466d4fe9fbbe59f21df` found:

- Generated shims embed both the runner path and the target repository path, although Git-based root discovery already exists.
- Application behavior is selected partly from repository basenames and lives in central Ruby subclasses.
- Shared agent-context links use absolute paths, while mutable per-workspace state is intentionally excluded from sharing.
- Port allocation hashes a lane key and probes current listeners; it does not reserve assignments for stopped workspaces.
- No-start handling is limited to one adapter, while other adapters can start their services during setup.
- Useful existing behavior includes isolated development/test databases, shared-database protections, selective cleanup and Cargo path-dependency wiring. Preserve these requirements without assuming their current implementation is the general contract.

Inspection of Portman also established that config roots own service, secret-block and egress registrations. A moved directory does not automatically rename those registrations. An integration must preserve intended running/stopped state and use supported operations rather than rewrite another daemon's persistence files.

Local operational inventories stay outside this repository. These observations do not certify a live migration procedure.

## Prior art

| Project | Relevant capabilities | Boundary to evaluate |
|---|---|---|
| [Worktrunk configuration](https://worktrunk.dev/config) and [hooks](https://worktrunk.dev/hook) | Repo-local and central per-project configuration, lifecycle hooks, worktree path templates and runtime variables | Reuse useful lifecycle/discovery patterns; hooks alone do not define TreeTender's requested resource ownership and selective cleanup contract. |
| [Git Worktree Runner](https://github.com/coderabbitai/git-worktree-runner) | Local Git configuration, shared config, file copying and lifecycle hooks | Compare its integration surface and invocation contract without assuming application/database equivalence. |
| [mise tasks](https://mise.jdx.dev/tasks/task-configuration.html) | Repository-local tasks, tools, execution directories and dependency ordering | A possible task integration, not the complete cross-repository resource registry. |
| [devenv processes](https://devenv.sh/processes) | Declarative environments/processes and automatic port allocation | Broader product scope; learn from resource handling without committing to replacing existing runtimes. |
| [Dev Container specification](https://containers.dev/implementors/json_reference/) | Portable container environment configuration and lifecycle commands | Container development can be an integration rather than a prerequisite. |
| [HanifCarroll/devtree](https://github.com/HanifCarroll/devtree) | Rust/MIT implementation of runnable worktrees, explicit file copies, setup commands, app state and URL providers | Relevant implementation to inspect. The reviewed config/source models setup/apps and fixed URL-provider choices rather than the desired general resource plan/teardown protocol. |

No external code has been copied or vendored. Review relevant implementation and licensing before selecting reusable code or dependencies.

## Name

TreeTender was selected after finding direct developer-tool conflicts for `devtree`, including the Rust project above. Initial `treetender` registry lookups found no crates.io/npm package record; that is a dated observation, not a reservation or public release commitment.
