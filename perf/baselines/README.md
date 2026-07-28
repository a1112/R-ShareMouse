# Reviewed performance baselines

`manifest.toml` is the only baseline index. A baseline JSON path passed directly
to `rshare-perf compare` is never authoritative. Missing entries, empty hashes,
placeholders, hash mismatches, malformed commits, and unavailable approval
verification all fail closed.

## Bootstrap or update

1. On the fixed runner, build with the pinned toolchain and `Cargo.lock`. Record
   all participating daemon, desktop, and `rshare-perf` binaries by role.
2. Run one immutable batch of exactly five complete runs. Do not discard failed
   or outlying runs and do not repair a batch with selective reruns.
3. If any metric has CV above 10%, archive that complete batch and rerun the
   entire five-run batch once with the same configuration. Preserve both. If the
   second batch is unstable, fail the workflow as infrastructure instability.
4. Canonicalize and SHA-256 the artifact bytes, then add the artifact and exact
   manifest entry in one dedicated pull request. The entry contains a 40-hex
   source commit and `approval_ref = "github-pr:OWNER/REPO#NUMBER"`.
5. Merge only after a reviewer other than the author submits `APPROVED`.

The enforcement workflow must fetch `manifest.toml` and the artifact from the
protected default branch. It must query the GitHub API and produce approval
evidence proving: default-branch protection, the PR is merged, a non-author
approval exists, and the reviewed diff contains the exact manifest entry and
artifact SHA-256. The evidence itself is SHA-256 fingerprinted. If the API,
branch protection, reviewed diff, or approval cannot be verified, comparison
fails; no offline or mocked success fallback is permitted.

The README describes the procedure but has no authority.
