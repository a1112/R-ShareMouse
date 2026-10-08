# Performance measurement

Performance gates consume versioned JSON artifacts, never console timing. Every
artifact records scenario parameters and their recursively canonical SHA-256,
seed, source state, role-keyed binary hashes, `Cargo.lock`, build profile,
sorted/deduplicated features, `RUSTFLAGS`, runner/toolchain/hardware
fingerprints, availability, measurement provenance, queue data, errors, RSS,
and verdict.

Strict comparison accepts exactly five complete runs from one immutable batch.
All runs retain failures and outliers and must agree on scenario/configuration,
seed policy, schema, build inputs, binary/lockfile policy, and runner. Required
counters are `overwrite`, `gap`, `duplicate`, `out_of_order`, and
`reliable_overflow`. `unsupported` and `not_run` never pass.

Each reported metric is reduced by the median across the five runs. Median
regression above 10% fails; p95/p99 regression above 15% fails. A metric CV
above 10% makes the whole batch unstable. One complete identical five-run retry
is allowed; both batches are preserved, individual runs are never replaced, and
a second unstable batch fails as infrastructure instability.

The hosted catastrophe workflow invokes the QUIC command with
`--validation-profile hosted-catastrophe`. This explicit profile keeps the
complete `available` artifact and applies the workflow's absolute delivery,
queue, and p99 ceilings even when a shared hosted runner reports
`unstable_after_one_complete_retry`. It does not change the fixed-runner CV
policy: the default profile still fails that result, and `unsupported` or
`not_run` artifacts never pass.

The declared QUIC matrix is 125 Hz for 10 seconds, 500 Hz for 10 seconds,
1000 Hz for 60 seconds, 1000 Hz for 60 seconds under diagnostics/status/audio/
bulk load, slow/fast peer isolation, and recovery from an exact 100 ms stall.
Until a fixed runner actually executes a scenario, the tool writes a
fail-closed `not_run` artifact. Real daemon IPC measurement is deferred until
Task 14 supplies the framed seam; no echo daemon and no hard-coded port 27435
may substitute for it.

Baselines are resolved only through `perf/baselines/manifest.toml` from the
protected default branch and require GitHub API approval evidence as documented
in `perf/baselines/README.md`.
