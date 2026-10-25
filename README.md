# TreeTender

Reproducible workspace lifecycles for humans, agents and their tools.

TreeTender is a planned Rust CLI for preparing, inspecting and cleaning up development workspaces. It will coordinate the files, database lanes and services a workspace needs through repository-local declarations and interchangeable integrations.

Git remains responsible for repositories and worktrees. Portman is the first intended service integration; other tools and user scripts should be able to participate through the same lifecycle contract. Editors, agents and tools such as Orca can invoke TreeTender through simple commands.

## Status

Private project, in design. This repository contains the product brief and working constraints; there is no executable or supported configuration format yet.

Read the [product brief](docs/brief.md) for the agreed direction, proposed first scope and open decisions. [Research](docs/research.md) records relevant prior art and the problems observed in the existing setup.

## Intended responsibilities

- Prepare explicitly selected local files and configuration.
- Provision isolated development/test database lanes.
- Coordinate service registrations and lifecycle through integrations.
- Inspect owned resources and report what is ready, missing or inconsistent.
- Plan selective teardown while preserving resources the user wants to keep.
- Track workspace identities and current locations without embedding machine paths in portable recipes.

Agent-facing operation is part of the initial design: noninteractive invocation, structured results, inspectable plans and explicit destructive actions.

## Boundaries

TreeTender will reuse Git and existing service tooling. It is not intended to replace a process supervisor, secret manager, editor or agent orchestrator. Existing workspaces and services remain unchanged while the new lifecycle contract is designed and verified.
