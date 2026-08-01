# Phase 1 Windows validation

Phase 1 is the fixed-runner acceptance gate for the control hot path and UI
state display. Its measurements are **single-machine loopback** results. They do
not prove wired two-machine latency, glass-to-glass latency, or the absolute
mouse/key targets reserved for Task 30.

## What runs where

`performance-smoke.yml` runs on GitHub-hosted Linux for every pull request and
push. It runs:

- `cargo test --workspace --locked`;
- `npm ci`, frontend Node tests, and the production frontend build;
- `cargo bench --workspace --no-run --locked`;
- catastrophe-only checks: 100,000 CPU-path operations within 5 seconds, framed
  IPC p99 at or below 100 ms sequentially and 200 ms at concurrency 8, and QUIC
  loopback at 125 Hz for 5 seconds with 100% reliable delivery, at least 99.9%
  datagram delivery, and p99 at or below 100 ms.

Hosted smoke does not compare commits, enforce fixed-runner UI/GPU timings, or
establish a performance baseline. Its deliberately wide ceilings catch severe
breakage only.

`performance-nightly-windows.yml` runs only on a self-hosted runner carrying all
four labels:

```text
self-hosted
Windows
X64
rshare-perf
```

The workflow rejects non-default-branch dispatches, verifies branch protection,
checks out the current protected default-branch head, installs only locked Rust
and npm dependencies, records the runner fingerprint/power plan/CPU affinity,
switches to the High performance plan, uses a run-specific `CARGO_TARGET_DIR`,
and invokes:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/perf/run-phase1.ps1 `
  -OutputDirectory C:\tmp\rshare-phase1-results `
  -Mode Strict `
  -WarmupSeconds 30
```

The original power plan is restored with an unconditional cleanup step.
Concurrent nightly runs are prohibited. Workflow code never writes, commits,
pushes, or otherwise updates a baseline.

## Exactly-five and instability policy

The Phase 1 runner creates one immutable batch id, warms for 30 seconds, then
runs exactly five complete same-configuration measurements for every required
Rust/QUIC/IPC and `@fixed-runner` UI scenario. IPC and UI are normalized into
the same repository-schema artifact used by QUIC, including commit, runner,
power-plan/affinity, binary, lockfile, warmup, raw-evidence, availability, and
verdict fields. QUIC preserves per-run latency samples (and slow-send samples
where applicable), IPC preserves every sequential/concurrent latency sample,
and UI preserves every paint and topology/status sample. Every raw
JSON/histogram remains in the output directory and every promotable primary
artifact references at least one hash-bound sidecar.

A run is complete only if the process succeeds, the artifact validates, and all
required metrics and counters are present. Failed, unavailable, unsupported, or
outlying runs are never removed or selectively replaced.

If any comparative latency/rate metric has coefficient of variation (CV) above
10%, the runner archives the entire unstable five-run batch and repeats the
entire five-run batch once with the identical configuration. Both batches are
retained. A second unstable batch fails as infrastructure instability. Stall
recovery is instead a per-run bounded correctness metric (every run must be at
or below 20 ms): its normal 1 us timer quantization around a near-zero loopback
result must not create a meaningless high CV.

## Reviewed baseline authority

Strict mode resolves a matching scenario/configuration and runner fingerprint
only through `perf/baselines/manifest.toml` read from the protected default
branch. It verifies:

1. the baseline artifact and configuration hashes;
2. the 40-hex source commit and matching report fingerprints;
3. the referenced baseline pull request is merged;
4. the latest review from an eligible non-author reviewer is `APPROVED`, is
   bound to the PR head, and the reviewer currently has repository write,
   maintain, or admin permission;
5. the reviewed diff contains the exact manifest entry, primary artifact, and
   every referenced batch sidecar;
6. the primary artifact and sidecars read directly from the approved PR head
   match every declared SHA-256.

GitHub API or branch-protection evidence being unavailable is a failure, not a
warning. Direct artifact paths, placeholder entries, hashes from the candidate
branch, and unreviewed files are not baseline authority.

Strict comparison requires median regression at or below 10% and p95/p99
regression at or below 15%, in addition to all correctness and queue bounds.
This comparison applies to QUIC, framed IPC, and UI-state artifacts. Missing or
unapproved matching baselines fail strict mode.

## Bootstrap is `PENDING_BASELINE`, never PASS

Bootstrap is a manual fixed-runner checkpoint used only when no reviewed
matching baseline exists:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/perf/run-phase1.ps1 `
  -OutputDirectory C:\tmp\rshare-phase1-bootstrap `
  -Mode Bootstrap `
  -WarmupSeconds 30
```

Bootstrap must still produce the same complete exactly-five-run artifact (and
perform the one permitted whole-batch retry when CV exceeds 10%), but its gate
status is `PENDING_BASELINE`. It must never emit PASS.

To promote a candidate:

1. commit the Task 20 implementation, ensure the worktree is clean, and run
   Bootstrap from that immutable implementation commit;
2. create a dedicated draft baseline PR so its real
   `github-pr:OWNER/REPO#NUMBER` reference exists before finalizing the
   manifest entries;
3. for all six generated scenarios (four QUIC scenarios, framed IPC, and
   desktop UI), copy every repository-shaped
   `perf/baselines/candidates/...` directory, including each canonical
   `report.json` and every raw sidecar envelope, without renaming or changing
   LF line endings;
4. copy all six `manifest-entry.toml.template` blocks into
   `perf/baselines/manifest.toml` and replace only their
   `github-pr:OWNER/REPO#NUMBER` placeholders with that draft PR reference;
5. verify every manifest primary/sidecar SHA-256 value against its copied file,
   then update the same dedicated PR with all six artifacts and exact entries;
6. obtain a non-author `APPROVED` review and merge that PR to the protected default
   branch;
7. update/rebase the implementation to that protected branch;
8. rerun the nightly command in Strict mode.

Only the final strict run against the reviewed immutable entry can pass Phase 1
and unlock Task 25. The scheduled nightly workflow never runs Bootstrap mode
and never auto-accepts a candidate.

## Loopback gate interpretation

All Phase 1 results and uploaded artifacts must be labeled `loopback`. The gate
requires:

- zero reliable loss/duplication and zero stuck modifiers;
- an ordered Shift and primary-mouse-button press/release sequence delivered
  through the real reliable QUIC lane;
- recovery from the declared 100 ms stall without stale replay;
- bounded outbound and inbound queues with no correctness loss; inbound
  evidence covers real-time latest-value replacement, reliable input, control
  and event mirrors, telemetry and event mirrors, bulk, protocol errors, and
  terminal releases; bulk is also mirrored through the authenticated manager
  event path so the daemon processes its message semantics instead of merely
  draining transport pressure;
- loaded QUIC p99 no more than 5 ms above unloaded, slow-peer fast-path p99 no
  more than 2 ms above fast-only, and 100 ms stall recovery within 20 ms;
- fixed-runner median regression no greater than 10%, and p95/p99 regression no
  greater than 15%;
- UI paint p95 at or below 16.7 ms and p99 at or below 33 ms;
- topology/status UI p95 at or below 50 ms and p99 at or below 100 ms;
- no healthy-stream dashboard or endpoint polling.

UI event-to-paint latency starts at the streamed event timestamp and ends in a
React passive effect for the render that committed the corresponding visual
state. For these asynchronous stream updates, the passive-effect task follows
the browser's committed-paint opportunity. The measurement therefore includes
WebSocket dispatch, JSON decoding, revision checks, store application,
RAF-batched continuous-state publication, React render/commit, and the browser
paint boundary without adding a harness-only animation frame. Pointer and all
100 ordered discrete transitions are measured independently. The production
Vite build is served for this test;
Node, Playwright, Chromium, headless mode, viewport, lockfile, test, and
Playwright-config identity are bound into the scenario configuration. The
browser environment also binds the Windows version/release, `NODE_OPTIONS`,
and WebGL vendor/renderer so GPU or runtime drift changes the configuration
identity. The gate
also requires all 100 discrete transitions to be applied in
order, the final state to be released, a minimum frame-sample count, zero
topology-projection commits during the pointer flood, and zero long tasks over
50 ms.

The 3/6/10 ms mouse targets and 8/15 ms reliable-input targets are informational
in Phase 1. They are not PASS criteria here because loopback does not include a
physical network, a second machine, or independently validated cross-machine
timing.

Task 30 is the only gate that may claim wired dual-machine absolute SLOs. It
must publish the physical-run evidence, raw samples, clock-correlation or
external measurement provenance, and p50/p95/p99 values. Until that artifact
exists, dual-machine and glass-to-glass claims remain `PENDING`.

## Runner operations checklist

Before enabling the `rshare-perf` label:

- keep Windows, firmware, drivers, Rust, Node, and runner hardware stable;
- disable unrelated scheduled jobs and interactive sessions;
- install GitHub CLI (`gh`) for fail-closed branch-protection and approval
  verification, and configure `PERF_BASELINE_GH_TOKEN` with repository/PR read
  plus Administration-read permission for branch-protection inspection;
- optionally set repository variable `RSHARE_PERF_AFFINITY_MASK` to the reviewed
  hexadecimal mask (for example `0xFF`); otherwise the runner records and
  reapplies its inherited mask;
- ensure the runner account may query/set power plans and query protected-branch
  and pull-request approval evidence;
- ensure enough free space for two full batches and 90-day workflow artifacts.

Both Bootstrap and Strict require a clean worktree before and after measurement;
the commit, lockfile, runner settings, and measured binary hashes must remain
identical across the run. This prevents a candidate from claiming the current
commit while measuring uncommitted or changing source. Playwright output is
written only below the batch artifact directory. The runner fingerprint
includes the active power-plan GUID and applied CPU-affinity mask.

The 100 ms stall harness drains through the same production
`LatestRealtimeReceiver::drain_latest` operation used immediately before
daemon injection.
Overwrite and stale-replay counters are derived from observed sequence
transitions; the gate does not use a harness-only filter or fixed success
counter.

After a run, verify the artifact contains the default-branch protection
evidence, runner fingerprint, machine settings, original/restored power plans,
every raw run and histogram, combined summary, baseline id/hash/approval
evidence, batch id, configuration hash, binary/lockfile hashes, and final
PASS/PENDING/FAIL verdict.
