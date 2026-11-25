# SOUL-10: Decide the supported distribution contract

Status: draft

## Summary

The repository currently publishes only three `.tar.xz` archives and one
`SHA256SUMS` file as `release-<sha>` prereleases. The documentation previously
described stable numbered releases, nightly builds, an installer, Homebrew,
`.deb` and `.rpm` packages, cargo-binstall, and a `develop` branch. Those
artifacts are not produced by the current release workflow.

The README now describes the tarball reality. The owner still needs to decide
whether tarball-only prereleases are the intended distribution contract or
whether the missing package and installer paths are supported products that
should be restored.

## Motivation

Installation is the first user-facing contract of the project. A distribution
path that does not exist sends users to stale upstream artifacts or makes the
fork look broken. The unused `packaging/nfpm.yaml` and
`scripts/package-linux.sh` make the intended answer ambiguous: they can build
packages, but no release step calls them.

## Design

Choose one contract and make every artifact, document, and release step agree
with it.

### Option A: tarball-only prereleases

Keep the current release workflow. Remove or archive the unused nfpm packaging
path, and make installation documentation point only at release archives and
source builds. This is the smallest contract and matches what already ships.

### Option B: restore platform packages as first-class artifacts

Build the `.deb` and `.rpm` artifacts in the release workflow with the existing
nFPM recipe, attach them to the same prerelease, and test installation from
those packages. Remove package-manager instructions that the repository does
not control, unless those channels are also owned and updated.

### Option C: restore an installer and mutable nightly channel

Recreate the installer as a versioned release artifact rather than a mutable
remote script, pin the archive and checksum it installs, and publish a clearly
labeled nightly only if there is an owner for its operational cost. This is the
largest contract and needs a trust design for script and checksum anchoring.

## Alternatives considered

- Leave the README describing upstream packages. Rejected: the current
  repository does not produce them.
- Delete the packaging path immediately. Rejected: restoring packages may be
  the owner's intended direction, and the path is evidence for that design.

## Open questions

1. Is this repository's supported distribution the fork's `release-<sha>`
   prerelease archives, the upstream project's numbered releases, or both?
2. Are `.deb` and `.rpm` packages worth supporting in this repository?
3. Should cargo, Homebrew, and cargo-binstall remain documented even when they
   resolve to upstream rather than this checkout?
4. Is a mutable nightly channel still wanted, and who owns its operational
   cost?

## Decision

Undecided. Recommendation: adopt Option A now because it matches the shipped
workflow, then restore Option B only if package installation is a real user
need. Do not restore Option C without a separate trust design.
