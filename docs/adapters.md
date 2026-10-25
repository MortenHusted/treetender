# Adapter contract

TreeTender delegates backend operations through versioned JSON requests over stdin and responses over stdout. Commands are argv arrays executed directly; shell execution requires explicitly selecting a shell. Adapter declarations are trusted executable code.

The core owns selection, dependency ordering, primary/shared protections, claims and operation history. Adapters own backend-specific inspection and effects for an exact target. A provider must return an exact instance and ownership token for managed cleanup; a convenient name is not sufficient proof.

## Script resources

```toml
[[resources]]
id = "storage"
kind = "data"
ownership = "owned"
target = "storage:tt_{workspace_id}"
depends_on = []
ports = []
[resources.adapter]
type = "script"
command = ["my-storage-adapter"]
capabilities = ["inspect", "prepare", "destroy"]
timeout_secs = 30
```

`{workspace_id}` and `{repository_id}` expand to stable IDs before planning. This is string substitution in the target, without shell evaluation. Use workspace IDs for isolated allocations and shared/borrowed declarations for existing defaults. Resource IDs are selectors within a workspace; targets identify provider allocations across the registry.

Requests have this shape; `workspace` includes `id`, `repository_id`, `path`, `git_dir`, `common_dir` and `primary`:

```json
{
  "version": 1,
  "action": "prepare",
  "workspace": {"id":"stable-workspace-id","repository_id":"stable-repo-id","path":"/checkout","git_dir":"/git-dir","common_dir":"/common-dir","primary":false},
  "resource_id": "storage",
  "target": "storage:tt_stable-workspace-id",
  "owner_token": "allocation-token-generated-by-treetender",
  "receipt": {"presence":"missing","instance":null,"owner_token":null,"handle":null}
}
```

Return exactly one JSON response on stdout; send bounded diagnostics to stderr. The response is versioned independently of the manifest:

```json
{
  "version": 1,
  "proof": {
    "presence": "present",
    "instance": "backend-generated-immutable-instance-id",
    "owner_token": "allocation-token-generated-by-treetender",
    "handle": {"namespace":"tt_stable-workspace-id"}
  }
}
```

Store the allocation token in backend metadata when preparing a new instance. Inspection must read that stored token and instance identity; echoing the request token invents ownership and defeats the protection. Before destruction, independently verify both the stored token and exact instance against the prior receipt. A database provider might use namespace metadata; a storage provider might use a prefix ownership object. Protect shared backing stores independently inside the provider.

| Action | Contract |
| --- | --- |
| `inspect` | Read only; return `present`, `missing` or `unknown`, with actual backend proof |
| `prepare` | Create only a missing isolated allocation; preserve existing foreign state |
| `start` | Start the exact verified service instance; do not start unrelated services |
| `stop` | Stop the exact verified service instance; retain persistent data |
| `destroy` | Verify instance/token and remove only that allocation; return `missing` |

Unsupported capabilities, unavailable inspection and ownership mismatches block cleanup. They never imply the resource is missing. Nonzero exit, malformed output, output above the limit or timeout fail the step. Completed receipts and provisional claims remain centrally available for recovery. Arbitrary provider effects are not transactional; retry must inspect first.

The `namespace_adapter` Rust example models independent database/storage namespaces in disposable files. Build it with `cargo build --example namespace_adapter`, set `TREETENDER_DEMO_ROOT` to a disposable directory, and put its binary on PATH. `examples/namespaces.toml` exercises generic retain/clean policy. This fixture is not a PostgreSQL client or a production storage adapter.

## Files and agent context

```toml
[[resources]]
id = "local-env"
kind = "file"
[resources.adapter]
type = "file"
source_root = "primary"
source = ".env"
destination = ".env"
mode = "copy"
```

The default source root is `workspace`; `primary` explicitly reads the main checkout. Destinations are always relative to the selected checkout. `copy` creates a private regular file; `link` creates a declared symlink. Existing destinations are preserved. Cleanup checks the recorded instance and contents, preserves edits and refuses symlink traversal. Use `kind = "agent"` for instructions/skills/tool configuration with the same rules. Parent directories created for preparation can remain after file cleanup.

## Portman

The built-in Portman adapter currently provides read-only observations of an explicit service through the daemon's length-prefixed JSON Unix-socket protocol. Put machine-specific socket paths in the private overlay. Declare existing services as `borrowed` or `shared`; TreeTender never implicitly adopts or destroys them.

```toml
[[resources]]
id = "web"
kind = "service"
ownership = "borrowed"
target = "portman:my-web"
[resources.adapter]
type = "portman"
service = "my-web"
socket = "/machine-specific/portman.sock"
```

A missing or empty service identifier is invalid. Inspection refuses ambiguous duplicate service names and unavailable or incompatible daemon responses. It does not synchronize configuration or start services. Managed service effects require a script adapter with real provider ownership proof.

Portman's CLI `up` synchronizes the entire repository configuration before starting the requested subset. It cannot safely be treated as an isolated single-resource command. TreeTender does not invoke `up`, an empty global selection or repository-wide forgetting on your behalf.
