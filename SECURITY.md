# Security policy

## Report a vulnerability privately

Use [GitHub private vulnerability reporting](https://github.com/MortenHusted/treetender/security/advisories/new). Do not put exploit details, credentials, private configuration or customer data in a public issue.

Include the affected commit or version, operating system, a minimal reproduction in a disposable workspace, expected impact and any workaround. Redact tokens, environment values and identifying paths from logs. Do not test against another person's workspaces or live services.

## Supported versions

TreeTender is experimental and has no published stable release. Security fixes target the latest default-branch code. Older commits and forks have no separate support commitment. A release will document its support policy when published.

## Security boundaries

- Repository tasks and script adapters are trusted executable code with the caller's permissions. TreeTender does not sandbox them. Review unfamiliar declarations before invoking provider inspection or applying a plan.
- Primary checkout protections, exact ownership receipts and shared references are intended to prevent accidental deletion. They do not authorize adopting arbitrary existing resources.
- Backend adapters must verify stored instance identity and allocation ownership themselves. Echoing an inspection request is not ownership evidence.
- Keep credentials in existing credential tooling. Portable manifests and provider receipts must not contain secret values; machine state and overlays remain private.

File traversal or replacement bugs, loss of ownership provenance, deletion of protected resources, leaked credentials and incorrect recovery from interrupted operations are security-relevant reports. Ordinary feature requests and unsupported provider capabilities belong in public issues.
