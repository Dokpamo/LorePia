# Repository governance

LorePia protects `main` as the integration and release source of truth.

## Changes to `main`

- Changes reach `main` through a pull request from a short-lived branch.
- The branch must be current with `main`, all configured CI and security checks must pass, and
  unresolved review conversations block merging.
- Force pushes and branch deletion are disabled for `main`.
- Release tags are immutable after creation.

## Review policy

The repository currently has one trusted maintainer. Requiring an approving review would prevent
that maintainer from merging because GitHub does not allow self-approval. Until a second trusted
maintainer is appointed, the required approval count remains zero and the maintainer must review
the final diff, test evidence, and security-sensitive permission changes before merging.

When a second trusted maintainer is appointed, the repository should require at least one approval
and CODEOWNER review for release workflows, Tauri capabilities, credential handling, and signing
configuration.

## Security reports

Potential vulnerabilities should be reported through GitHub private vulnerability reporting as
described in `SECURITY.md`. Public issues should not contain exploit details or secrets.

## Releases

Unsigned workflow artifacts are test candidates only. A public release requires protected signing,
platform signature verification, checksums, an SBOM, provenance attestations, and clean-machine
installation evidence. The release workflow remains fail-closed until those requirements are met.
