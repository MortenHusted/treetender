# TreeTender

- Read `docs/brief.md` before design or implementation; distinguish agreed direction from proposed scope and open decisions.
- The product is Rust with CLI, TUI and agent access from the start. Keep one shared lifecycle interface usable by noninteractive agents and human callers.
- Resource registration and accounting per tree/lane are the core; backend mutations are optional. Reuse existing resource managers and report accounting-interface gaps rather than assuming a new database driver is required.
- Reuse Git for worktree metadata and existing tools for service supervision and credentials. Portman is an integration, not a required runtime.
- Keep portable repository configuration, private machine inputs and runtime resource ownership distinct.
- Do not start, stop, drop, migrate or adopt live external resources while developing or testing without authorization for those specific effects. Use disposable fixtures by default.
- Do not introduce tracked configuration into another team repository without explicit opt-in.
- Keep `AGENTS.md` canonical and `CLAUDE.md` as its symlink. Put procedures and product decisions in the relevant documentation rather than expanding this file.
