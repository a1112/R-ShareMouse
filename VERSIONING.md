# R-ShareMouse version management

## Current authority

The existing source/package values below are preserved. They are not a claim that a product is stable or that these source commits were released. Independent applications, libraries, build tools and protocol/schema numbers are not reset to one repository-wide version.

- `rust-workspace`: `0.1.0` from `Cargo.toml` (workspace.package.version); explicit local bump supported. Rust workspace package version; inherited crate declarations remain source bindings Tauri application packaging version is synchronized with its own Cargo package.
- `node-apps-rshare-desktop-frontend`: `0.0.1` from `apps/rshare-desktop-frontend/package.json` (/version); explicit local bump supported. Node package version; independent unless explicitly coupled by the application package below.

The machine-readable inventory is `versioning.json`. Cargo workspace inheritance and local lock entries are bound explicitly. Node lock root fields are distinct from registry dependency versions. Tauri versions are bound to their own package; different frontend numbers remain independent.

## Check and change

Python 3.11 or newer; standard library only. Run from the repository root.

```sh
python3 scripts/version_manager.py show
python3 scripts/version_manager.py check
python3 scripts/test_versioning.py -v
```

Choose a single unit. Preview is non-mutating; apply is explicit and should always include the observed current value:

```sh
python3 scripts/version_manager.py bump patch --unit rust-workspace
python3 scripts/version_manager.py bump patch --unit rust-workspace --expect 0.1.0 --apply
python3 scripts/version_manager.py check
```

`set NEW --unit NAME` also previews; add `--expect CURRENT --apply` only after review. For prereleases use `bump prerelease`; stable promotion requires explicit `--allow-stable`. The helper does not run Git, tags, package managers, release hooks, signing, installers or network requests. All source edits are validated before any write.

Check-only units intentionally refuse bump/set because native dependency resolution, existing tooling or historical/test-only ownership must be respected. Incoming local Cargo version requirements and Python workspace locks are not approximated by search-and-replace. Use the project’s native packaging process, update constraints/locks together, then rerun these checks.

## Release history and channels

- No release entry was returned in the repository release inventory reviewed on 2026-10-12. Absence here does not prove the project was never distributed.
- Verified central distribution: [0.1.0-preview.1](https://github.com/a1112/R-Box-Releases/releases/tag/r-sharemouse-v0.1.0-preview.1) (prerelease), tag `r-sharemouse-v0.1.0-preview.1`. This distribution identity is separate from crate/frontend source version fields.

This change creates no tag, release, publication, deployment or release date. Record future changes under Unreleased first; use the existing release process and tag namespace only after platform validation and explicit release approval.

## Scope and validation

The added CI checks metadata consistency and dependency-free version-manager safety only, with `contents: read`. Apply/idempotency/stale-source/mismatch checks use temporary fixtures. They do not establish that a native app builds, signs, installs or works on macOS/Windows/Linux. No native packaging or device acceptance was run for this adoption.

Generated Apple projects, vendored/upstream material, historical evidence, protocol/schema/database numbers and dependency versions remain outside product bumps. Existing runtime version mechanisms are retained; there is no UI redesign.

## Adoption base

Source snapshot: [repair/macos-native-lan-20261012 @ 8a2082816948](https://github.com/a1112/R-ShareMouse/commit/8a20828169488b2550db1a991b2011cc0fddedc2). Default branch `main` and all existing repair/tag refs are retained.

Divergent repair chains were not merged or cherry-picked: `codex/fix-deps-sha2-0.11`.
