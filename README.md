# TreeTender

Reproducible workspace lifecycles for humans, agents and their tools.

TreeTender is a planned Rust tool with CLI, TUI and agent access for preparing, inspecting and cleaning up development workspaces. It will coordinate the files, database lanes, builds and services a workspace needs through repository-local declarations and interchangeable integrations.

Git remains responsible for repositories and worktrees. Portman is the first intended service integration; other tools and user scripts should be able to participate through the same lifecycle contract. Editors, agents and tools such as Orca can invoke TreeTender through simple commands.

## Status

Private project, in design. This repository contains the product brief and working constraints; there is no executable or supported configuration format yet.

Read the [product brief](docs/brief.md) for the agreed direction, proposed first scope and open decisions. The [lifecycle model](docs/lifecycle.md) explores operations and resource ownership. [Research](docs/research.md) records relevant prior art and the problems observed in the existing setup.

## Intended responsibilities

- Prepare explicitly selected local files and configuration.
- Provision isolated persistent resources, such as development/test database lanes or storage namespaces.
- Run declared workspace builds/checks and track their artifacts.
- Coordinate service registrations and lifecycle through integrations.
- Inspect owned resources and report what is ready, missing or inconsistent.
- Plan selective teardown while preserving resources the user wants to keep.
- Track workspace identities and current locations without embedding machine paths in portable recipes.
- Retain checkout/data at session end and make explicit cleanup easy.

CLI, TUI and agent-facing operation share one lifecycle model from the start. Noninteractive invocation, structured results, inspectable plans and explicit destructive actions are initial requirements, alongside TreeTender's own build/test/package workflow.

## Boundaries

TreeTender will reuse Git and existing service tooling. It is not intended to replace a process supervisor, secret manager, editor or agent orchestrator. Existing workspaces and services remain unchanged while the new lifecycle contract is designed and verified.
