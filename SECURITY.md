# Security

## Supported versions

Mutagen is in Phase 0 (repository bootstrap). No release is issued and no
version is maintained for security support yet.

## Reporting a vulnerability

Do **not** report security vulnerabilities in public issues.

Email maintainers via the GitHub security advisory feature:
[Report a vulnerability](https://github.com/z-chenhao/Mutagen/security/advisories/new)

We aim to acknowledge reports within 7 days and will coordinate on a fix
and disclosure timeline.

## Current attack-surface note

The Phase 0 codebase contains no network I/O, no untrusted-input parsing
beyond local CLI arguments, no `unsafe` code (workspace-forbidden), and no
plugin loading. The meaningful security design work — isolation for
evolvable/plugin components — is a tracked open research question in
[`docs/evolution.md`](docs/evolution.md), not a solved problem.
