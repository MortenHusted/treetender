# Contributing

Focused bug reports, reproducible examples and design discussions are welcome. For substantial changes, open an issue first so the intended behavior can be agreed before implementation.

By contributing, you offer your contribution under the project's [MIT License](LICENSE). You retain copyright in your original contributions; no copyright assignment is required. Include applicable licenses and notices for any third-party material.

Human and AI-assisted contributions need the same evidence. Explain the problem, the resulting behavior and how it was verified. You are responsible for reviewing what you submit, including generated code and any third-party material.

## Development

Build from a clean checkout with Rust, Git and Python 3 available. Python is used only by disposable test providers.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --release --locked
cargo package --locked --allow-dirty
```

CI checks macOS and Linux. Keep the lockfile committed. Do not edit a lockfile by hand to silence an advisory or a failing check.

## Changes to lifecycle behavior

CLI, TUI and agent operations share one planner and executor. Preserve primary/shared protections, ownership proof, retention selections and operation provenance. Keep backend-specific work in adapters rather than teaching the core application-specific database names or checkout paths.

Use temporary Git repositories and disposable providers for tests. Cover the failure or safety boundary a change affects, including interrupted effects when relevant. Do not start, stop, adopt or destroy a contributor's live resources as part of verification. Never commit local overlays, registry contents or credentials.

In a pull request, state what changes for callers and provide reproducible checks. Document interface changes. Include a focused regression when repairing behavior, rather than tests that merely repeat implementation details.

Follow the [code of conduct](CODE_OF_CONDUCT.md). Report vulnerabilities using [SECURITY.md](SECURITY.md).
