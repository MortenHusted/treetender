# Changelog

## Unreleased

- Add adapter-free passive resource declarations with purpose/manager notes, CLI/TUI accounting and agent discovery. Passive references never allocate, observe or mutate backend resources and remain protective during cleanup.

- Initial Rust CLI, TUI and structured agent interface.
- Central registry for Git identities, resource ownership, protected references and operation history.
- Explicit file copy/link resources and versioned JSON adapters for user-provided backends.
- Prepare, build, check, start and stop lifecycles, with declared artifact freshness and retain-by-default stop behavior.
- Reviewed cleanup and non-destructive forget, with primary/shared safeguards and interrupted-effect recovery.
- Read-only Portman service observation and disposable database/storage namespace examples.
- MIT license, public contribution and security guidance, and Linux/macOS CI.
