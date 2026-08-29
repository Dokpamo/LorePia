# Security policy

## Reporting

Please report suspected vulnerabilities through a private GitHub Security Advisory for this
repository. Do not include credentials, private conversation data, or a working exploit in a
public issue.

## Dependency exceptions

Security scan exceptions must identify an owner, a concrete reason, and an expiry date. Expired
exceptions fail review and must be removed, renewed with current evidence, or replaced by a safe
dependency update.

The current `cargo-deny` exceptions expire on **2026-11-30** and are owned by **Dokpamo**:

| Advisory set | Scope | Reason | Required follow-up |
| --- | --- | --- | --- |
| `RUSTSEC-2024-0411` through `RUSTSEC-2024-0420`, `RUSTSEC-2024-0370` | Tauri Linux GTK3 dependency chain | Unmaintained transitive crates; no safe direct upgrade | Recheck each Tauri release and remove when its Linux stack no longer requires GTK3 |
| `RUSTSEC-2025-0075`, `0080`, `0081`, `0098`, `0100` | Tauri `urlpattern` dependency chain | Unmaintained transitive `rust-unic` crates; no safe direct upgrade | Recheck each Tauri release and remove after the upstream replacement lands |

These exceptions cover maintenance-status advisories only. New vulnerability advisories remain
blocking in CI.
