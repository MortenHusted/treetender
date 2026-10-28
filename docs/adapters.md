# Adapter contract

Adapters connect existing tools to optional observation and lifecycle capabilities. TreeTender's resource registry accounts for lane associations independently of invoking those capabilities. A new database driver is not required merely to register an externally managed database.

Omit `adapter` to declare a passive resource: no command or connection is attempted, no allocation/port claim is created, and lifecycle effects exclude it. An explicit nonempty `target` is required; `description` and `manager` are optional informational notes. All ownership modes remain protected without an adapter. See the [inventory example](../examples/inventory.toml) and [registry semantics](registry.md). An observation adapter is optional when actual backend evidence is wanted. External receipt import is not implemented.

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

`{workspace_id}` and `{repository_id}` expand to stable IDs before planning. `{workspace_name}` expands to the workspace's registered name (lowercase letters, digits and hyphens, safe in hostnames) and `{workspace_ident}` to the same name with underscores (safe in database names). `{port:NAME}` expands to the workspace's lease for a declared `[ports.NAME]`. Names are fixed at first registration, so these stay stable when a checkout moves. This is string substitution in the target, without shell evaluation. Use workspace placeholders for isolated allocations and shared/borrowed declarations for existing defaults. Resource IDs are selectors within a workspace; targets identify provider allocations across the registry.

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

Adapter commands and tasks also receive `TREETENDER_WORKSPACE_ID`, `TREETENDER_WORKSPACE_NAME`, `TREETENDER_WORKSPACE_IDENT`, `TREETENDER_WORKSPACE_PATH`, `TREETENDER_REPOSITORY_ID` and one `TREETENDER_PORT_<NAME>` per port lease in their environment; the request's `workspace` object carries the same `name`.

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

Inspection requests deliberately omit the expected owner token and instance from the prior proof. The prior handle remains available as a locator; the provider must return independently stored ownership evidence. Handles must be deterministic locators, without volatile timestamps, PIDs or health counters that would invalidate every reviewed plan. A changed owner/instance or unknown provider state blocks effects.

An external provider unexpectedly reporting a previously prepared allocation missing preserves its claim. Verify the provider account/endpoint/context before explicitly forgetting it. Intent-specific receipts allow retry after prepare failed before creation or after an intended destroy took effect. Forget retains the backend instance and an audited copy of the receipt; it is not resource destruction.

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

The default source root is `workspace`; `primary` explicitly reads the main checkout. Destinations are always relative to the selected checkout. `copy` creates a private regular file; `link` creates a declared symlink to a source file or directory; `template` creates a private copy of a UTF-8 source with `{workspace_id}`, `{repository_id}`, `{workspace_name}`, `{workspace_ident}` and `{port:NAME}` substituted. Other braces, as in YAML, TOML or ERB, are left as written; a `{port:NAME}` without a declared lease blocks the plan. Set `optional = true` to skip a resource whose source does not exist instead of blocking, for example agent context that only some checkouts have. Existing destinations are preserved. Cleanup checks the recorded instance and contents, preserves edits and refuses symlink traversal. Use `kind = "agent"` for instructions/skills/tool configuration with the same rules. Parent directories created for preparation can remain after file cleanup. A rendered template is not updated when its inputs change later: clean and prepare it again. See the [lane overlay example](../examples/lane-overlay.toml) for links, templates and a build directory declared once for every lane.

## Owned directories

```toml
[[resources]]
id = "cargo-target"
kind = "file"
[resources.adapter]
type = "directory"
path = "target"
```

A directory resource owns one directory inside the checkout: Cargo `target/`, `node_modules/`, `.venv/` or another per-lane build or dependency tree. Its target is `dir:{workspace_id}:{path}`. Prepare creates the directory and writes `<path>/.treetender-owner.json` with the owner token and an instance ID generated at creation. Inspection reads that marker; the request token is never echoed. A directory without a marker is foreign state: prepare blocks instead of claiming it.

Cleanup captures the directory into private storage beside it, verifies the captured marker against the receipt and then removes that tree. Removal is descriptor-relative, never follows symlinks, stops at a nested `.git` and refuses to cross into another filesystem. The marker is removed last, so an interrupted cleanup still proves the same instance and resumes. A mismatched capture is restored without replacing anything that appeared at the path meanwhile. Cleanup also refuses a directory that contains tracked files.

Existing output is claimed only through a reviewed adopt:

```sh
treetender plan adopt --only cargo-target --out adopt.json
treetender apply adopt.json
```

Adopt requires an explicit selection, a linked (non-primary) workspace and a directory that is a real directory, ignored by Git and free of tracked files. The plan records the directory's device and inode, so a directory replaced after review makes the plan stale. Apply writes the marker; the contents are untouched until an explicit clean. A directory that already has a receipt must be forgotten before it can be adopted again.

Tools such as `cargo clean` or `npm ci` delete and recreate the whole directory, which removes the marker. Run `prepare` before building to recreate an owned directory. Once a build has recreated it without the marker, it is no longer the recorded instance: prepare refuses it, and clean retires the old receipt without deleting anything, after which the directory can be adopted after review. `status --usage` reports the allocated bytes of present directory resources, counting a hard-linked file once; it walks the trees, so it is opt-in, and it never enters plans or proofs. The figure is what the tree occupies, not what cleaning it frees: blocks it shares with files outside it still count in full. That covers copy-on-write clones (APFS `cp -c`, reflinks on Btrfs or XFS), so a tree cloned from another lane counts in full in both, and files hard-linked from a package store such as pnpm's. Shared caches such as a common `CARGO_TARGET_DIR` or sccache stay `shared` or `borrowed` declarations.

## PostgreSQL databases

```toml
[[resources]]
id = "dev-db"
kind = "data"
[resources.adapter]
type = "postgres"
database = "app_development_{workspace_ident}"
service = "local"   # or host/port/user; never a password
```

A postgres resource owns one database on a server it never owns. The adapter runs `psql` (override with `command`), so no database driver is built in and credentials come from libpq: a service file entry, a password file or `PG*` variables. Its target is `postgres:<service or host:port>/<database>`. The database name accepts workspace placeholders and must be 1–63 lowercase letters, digits or underscores.

Prepare runs `CREATE DATABASE` and stores a JSON marker with the owner token and an instance ID as the database comment. Inspection reads the OID and that comment; a database without the marker is foreign state and prepare blocks. Clean re-reads the marker, then, in the same psql session, drops the database only if its OID and comment still match (`DROP DATABASE … WITH (FORCE)`, PostgreSQL 13 or later). `DROP DATABASE` cannot run inside a transaction, so a database replaced in the instant between that check and the drop is not detected. A recorded database that unexpectedly disappears keeps its claim until a reviewed `forget`.

Existing lane databases can be adopted with a reviewed `adopt --only RESOURCE` when their name contains this workspace's `{workspace_ident}`, as evidence that they are lane-specific; the plan freezes the OID. Copies that a framework derives from a lane database, such as Rails' parallel test databases, carry no marker and are not dropped.

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

A missing or empty service identifier is invalid. Inspection refuses ambiguous duplicate service names and unavailable or incompatible daemon responses. It does not synchronize configuration or start services.

Portman's CLI `up` synchronizes the entire configuration root it finds before starting it, so TreeTender never runs it in a repository's own root. A lane-owned root is different: everything in it belongs to the lane.

### Portman lane roots

```toml
[[resources]]
id = "services"
kind = "service"
[resources.adapter]
type = "portman_root"
root = "tmp/workspace/portman"

[[resources]]
id = "services-config"
kind = "file"
depends_on = ["services"]
[resources.adapter]
type = "file"
source_root = "primary"
source = "portman.lane.template.toml"
destination = "tmp/workspace/portman/portman.local.toml"
mode = "template"
```

A `portman_root` resource owns a directory inside the checkout and every service Portman registers under that exact root. Portman service names are global, so the rendered config should prefix them with `{workspace_name}`; its `dir` and `env_files` are relative to the root. The directory carries the same ownership marker as a [directory resource](#owned-directories), and adopt works the same way.

- `prepare` creates the root directory. Render its config with a template resource that depends on it.
- `start` requires `portman.toml` or `portman.local.toml` in the root itself, so the CLI cannot walk up to an ancestor's config, then runs `portman up` there.
- `stop` asks the daemon for the services whose root is exactly this one and stops them by name; it never sends an empty selection, which Portman reads as every service.
- `clean` runs `portman down --forget --root <root>`, confirms that no service remains registered for the root, then removes the directory.

`socket` defaults to the socket Portman's CLI uses for the current user; set an absolute path to override it, and `command` to run a different `portman`. Clean before removing the checkout: a root whose checkout is already gone leaves its registrations for `portman down --forget --root`.
