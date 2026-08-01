# Low-Latency Control and Display Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a bounded, measurable control path that meets wired-LAN input latency targets, replace desktop polling with a revisioned state stream, and add an isolated Windows 1080p60 hardware video path without allowing media load to delay control.

**Architecture:** The daemon remains authoritative but moves hot work into single-owner actors: semantic input ingress, `InputRouter`, per-peer QoS transport handles, ordered `InputInjection`, and `StateAggregator`. Local UI uses framed IPC plus one versioned snapshot/delta stream, while video runs through a new `rshare-media` crate and a separate authenticated QUIC connection with bounded newest-frame queues and a native D3D11 presentation surface.

**Tech Stack:** Rust 2021, Tokio, Quinn/QUIC, serde/bincode, Criterion, React 18, Tauri 2, Windows KMDF, Windows Graphics Capture, DXGI Desktop Duplication, D3D11, Media Foundation H.264, Node test runner.

---

## Execution Rules

- Work only in the `codex/low-latency-control-display` worktree.
- Follow `@superpowers:test-driven-development` for every behavior change.
- In every RED step, wire a new test file into the crate/module tree before running it. A filtered command is valid only when Cargo/Node reports at least one matching test; zero discovered/run tests is a failed RED step, not a pass.
- If any test fails unexpectedly, stop and use `@superpowers:systematic-debugging`; do not stack speculative fixes.
- Before each phase-gate or completion claim, use `@superpowers:verification-before-completion`.
- Keep commits limited to one task. Do not mix media, UI, and control changes.
- Do not delete or reuse the main worktree's untracked `target/` directories.
- On the current Windows machine, `G:` has less than 1 GiB free. Before Cargo commands use:

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\rshare-low-latency-target'
```

- Preserve reliable ordering for key, mouse-button, wheel, gamepad-button, and session barriers. Only continuous mouse/gamepad state may be overwritten.
- Never hold a connection registry, daemon state, or transport map lock across `.await`.
- Never format strings, serialize JSON, or broadcast diagnostics on the input hot path.
- Never route media payloads through `rshare_core::Message`, daemon JSON IPC, or the control QUIC connection.

## Delivery Map and Gates

1. **Phase 1A — Measurement:** Tasks 1-2.
2. **Phase 1B — Control hot path:** Tasks 3-13.
3. **Phase 1C — State display and IPC:** Tasks 14-20.
4. **Phase 2A — Media skeleton:** Tasks 21-24.
5. **Phase 2B — Windows 1080p60:** Tasks 25-28.
6. **Phase 2C — Recovery, CI, and dual-machine acceptance:** Tasks 29-30.

Do not start Task 25 until the Phase 1 gate in Task 20 passes. Do not claim the video SLO until the physical two-machine run in Task 30 has produced an artifact containing raw samples and p50/p95/p99 values.

The first strict fixed-runner baseline is an explicit bootstrap checkpoint, not an auto-accept loophole. After Task 20 can generate the complete exactly-five-run artifact, pause behavior work, publish the candidate artifact plus manifest entry in a dedicated reviewed baseline PR, merge it to the protected default branch, then update/rebase this worktree and rerun Task 20 against that immutable reviewed entry. Only that second strict run can pass Phase 1 and unlock Task 25. Missing review infrastructure leaves Phase 1 `PENDING`; it does not permit media implementation to bypass the gate.

### Task 1: Pin the Measurement Runtime and Add Allocation-Free Local Stage Metrics

**Files:**
- Modify: `.gitignore`
- Modify: `Cargo.toml`
- Track: `Cargo.lock`
- Create: `rust-toolchain.toml`
- Create: `.node-version`
- Modify: `crates/rshare-core/Cargo.toml`
- Create: `crates/rshare-core/src/perf.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Create: `crates/rshare-core/benches/perf_record_alloc.rs`
- Test: `crates/rshare-core/src/perf.rs`

**Step 1: Write failing clock-domain and percentile tests**

Add tests that forbid subtracting monotonic values from different processes:

```rust
#[test]
fn cross_clock_domain_duration_is_rejected() {
    let sender = MonotonicStamp::new(ClockDomainId(1), 100);
    let receiver = MonotonicStamp::new(ClockDomainId(2), 180);
    assert_eq!(
        receiver.checked_duration_since(sender),
        Err(MonotonicTimeError::ClockDomainMismatch {
            earlier: ClockDomainId(1),
            later: ClockDomainId(2),
        })
    );
}

#[test]
fn same_domain_clock_regression_is_rejected() {
    let earlier = MonotonicStamp::new(ClockDomainId(1), 180);
    let later = MonotonicStamp::new(ClockDomainId(1), 100);
    assert!(matches!(
        later.checked_duration_since(earlier),
        Err(MonotonicTimeError::ClockRegression { earlier_us: 180, later_us: 100 })
    ));
}

#[test]
fn sender_and_receiver_stages_report_only_local_durations() {
    let sender = SenderStageStamps::fixture(ClockDomainId(1), 100, 120, 150, 180);
    let receiver = ReceiverStageStamps::fixture(ClockDomainId(2), 20, 30, 45);
    assert_eq!(sender.capture_to_route_us(), Some(50));
    assert_eq!(sender.capture_to_transport_us(), Some(80));
    assert_eq!(receiver.receive_to_inject_us(), Some(25));
}

#[test]
fn histogram_overflow_is_counted_not_silently_dropped() {
    let mut histogram = RollingLatencyHistogram::new(100).unwrap();
    histogram.record(101);
    assert_eq!(histogram.report().overflow, 1);
}

#[test]
fn unobserved_stage_is_unavailable_not_zero() {
    let report = LatencyReport::empty();
    assert_eq!(report.p50_us, None);
    assert_eq!(report.samples, 0);
}
```

**Step 2: Run the focused test and verify it fails**

Run:

```powershell
cargo test -p rshare-core perf::tests -- --nocapture
```

Expected: FAIL because `perf` and the tested types do not exist.

**Step 3: Pin the currently verified toolchain and lockfile**

Remove only the `Cargo.lock` ignore entry and track the existing workspace lock. Add:

```toml
# rust-toolchain.toml
[toolchain]
channel = "1.94.1"
profile = "minimal"
components = ["clippy", "rustfmt"]
```

and:

```text
# .node-version
24.15.0
```

These versions are the versions verified in this worktree. Changing either later requires an intentional dependency/performance-baseline update.

**Step 4: Add local-only stage types and an actor-owned histogram**

Add `hdrhistogram = "7.5.4"` and implement:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ClockDomainId(pub u64);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonotonicStamp {
    pub domain: ClockDomainId,
    pub value_us: u64,
}

#[derive(Debug, Clone, Copy, thiserror::Error, PartialEq, Eq)]
pub enum MonotonicTimeError {
    #[error("cannot compare clock domains {earlier:?} and {later:?}")]
    ClockDomainMismatch {
        earlier: ClockDomainId,
        later: ClockDomainId,
    },
    #[error("monotonic clock regressed from {earlier_us}us to {later_us}us")]
    ClockRegression {
        earlier_us: u64,
        later_us: u64,
    },
}

impl MonotonicStamp {
    pub const fn new(domain: ClockDomainId, value_us: u64) -> Self {
        Self { domain, value_us }
    }

    pub fn checked_duration_since(
        self,
        earlier: Self,
    ) -> Result<u64, MonotonicTimeError> {
        if self.domain != earlier.domain {
            return Err(MonotonicTimeError::ClockDomainMismatch {
                earlier: earlier.domain,
                later: self.domain,
            });
        }
        if self.value_us < earlier.value_us {
            return Err(MonotonicTimeError::ClockRegression {
                earlier_us: earlier.value_us,
                later_us: self.value_us,
            });
        }
        Ok(self.value_us - earlier.value_us)
    }
}

pub struct SenderStageStamps {
    pub captured: MonotonicStamp,
    pub ingress_enqueued: MonotonicStamp,
    pub router_dequeued: MonotonicStamp,
    pub transport_enqueued: Option<MonotonicStamp>,
}

pub struct ReceiverStageStamps {
    pub received: MonotonicStamp,
    pub injection_started: Option<MonotonicStamp>,
    pub injection_completed: Option<MonotonicStamp>,
}

pub struct LatencyReport {
    pub samples: u64,
    pub p50_us: Option<u64>,
    pub p95_us: Option<u64>,
    pub p99_us: Option<u64>,
    pub max_us: Option<u64>,
    pub overflow: u64,
}
```

`RollingLatencyHistogram` preallocates on construction, is owned by one low-priority actor, and counts values above its configured bound. It must not silently clamp or report missing data as zero.

Never add `capture_to_inject_us`: sender and receiver monotonic domains are not comparable. End-to-end reporting combines local sender/receiver durations with request/ack sequence and RTT estimates; true glass-to-glass remains external measurement.

**Step 5: Add a failing allocation harness**

Add `dhat = "0.3.3"` as a dev dependency and a custom bench that initializes and warms the recorder before measuring 100,000 `record()` calls:

```rust
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn main() {
    let mut histogram = RollingLatencyHistogram::new(1_000_000).unwrap();
    for value in 1..=10_000 {
        histogram.record(value);
    }
    let profiler = dhat::Profiler::builder().testing().build();
    for value in 1..=100_000 {
        histogram.record(value);
    }
    let stats = dhat::HeapStats::get();
    assert_eq!(stats.total_blocks, 0, "recording allocated after warmup");
    drop(profiler);
}
```

Register the custom executable harness in `crates/rshare-core/Cargo.toml` so Cargo runs `main` instead of an empty libtest harness:

```toml
[[bench]]
name = "perf_record_alloc"
harness = false
```

The harness prints and asserts that exactly 100,000 measured `record()` calls ran.

**Step 6: Run focused tests and allocation harness**

Run:

```powershell
cargo test -p rshare-core perf::tests
cargo bench -p rshare-core --bench perf_record_alloc
cargo test -p rshare-core
```

Expected: all tests PASS and the warmed 100,000-record section performs zero allocations.

**Step 7: Commit**

```powershell
git add .gitignore Cargo.toml Cargo.lock rust-toolchain.toml .node-version crates/rshare-core/Cargo.toml crates/rshare-core/src/lib.rs crates/rshare-core/src/perf.rs crates/rshare-core/benches/perf_record_alloc.rs
git commit -m "perf: add reproducible local stage metrics"
```

### Task 2: Add a Reproducible Performance Report and Comparison Tool

**Files:**
- Modify: `Cargo.toml`
- Create: `tools/rshare-perf/Cargo.toml`
- Create: `tools/rshare-perf/src/main.rs`
- Create: `tools/rshare-perf/src/report.rs`
- Create: `tools/rshare-perf/src/compare.rs`
- Create: `tools/rshare-perf/src/control.rs`
- Create: `tools/rshare-perf/src/quic.rs`
- Create: `tools/rshare-perf/src/dual.rs`
- Create: `perf/budgets/hosted-smoke.toml`
- Create: `perf/budgets/windows-fixed.toml`
- Create: `perf/budgets/windows-dual.toml`
- Create: `perf/budgets/windows-media.toml`
- Create: `perf/baselines/README.md`
- Create: `perf/baselines/manifest.toml`
- Create: `perf/baselines/schema.json`
- Modify: `crates/rshare-core/Cargo.toml`
- Modify: `crates/rshare-net/Cargo.toml`
- Create: `crates/rshare-core/benches/control_hot_path.rs`
- Create: `crates/rshare-net/benches/quic_loopback.rs`
- Create: `docs/performance/README.md`

**Step 1: Write failing report/comparison tests**

Require:

- five-run comparisons use the median of each reported metric;
- strict comparison accepts exactly five complete runs from one batch, never a selectively repaired subset;
- every run has the same scenario/configuration hash and contains every required metric/counter;
- runner fingerprint mismatch is an error;
- missing enforced baseline is an error;
- a missing, unhashed, hash-mismatched, or unapproved manifest entry is an error;
- median regression over 10% fails;
- p95/p99 regression over 15% fails;
- coefficient of variation over 10% is `unstable`, not pass;
- `unsupported`/`not_run` cannot satisfy a gate.

```rust
#[test]
fn five_run_comparison_rejects_material_p99_regression() {
    let baseline = five_runs("runner-a", [200, 201, 199, 202, 198]);
    let candidate = five_runs("runner-a", [240, 239, 241, 238, 242]);
    let verdict = compare(&baseline, &candidate, ComparisonPolicy::strict()).unwrap();
    assert_eq!(verdict.status, VerdictStatus::Fail);
    assert_eq!(verdict.regressions[0].metric, "p99_us");
}

#[test]
fn runner_fingerprint_mismatch_is_error() {
    let error = compare(
        &five_runs("runner-a", [100; 5]),
        &five_runs("runner-b", [100; 5]),
        ComparisonPolicy::strict(),
    ).unwrap_err();
    assert!(matches!(error, CompareError::RunnerMismatch { .. }));
}

#[test]
fn strict_comparison_requires_exactly_five_complete_same_config_runs() {
    let mut candidate = five_runs("runner-a", [100; 5]);
    candidate.runs.pop();
    assert!(matches!(
        compare(
            &five_runs("runner-a", [100; 5]),
            &candidate,
            ComparisonPolicy::strict(),
        ),
        Err(CompareError::IncompleteBatch { expected: 5, actual: 4 })
    ));

    let mut candidate = five_runs("runner-a", [100; 5]);
    candidate.runs[4].scenario_config_sha256 = "different".into();
    assert!(matches!(
        compare(
            &five_runs("runner-a", [100; 5]),
            &candidate,
            ComparisonPolicy::strict(),
        ),
        Err(CompareError::ScenarioConfigMismatch { .. })
    ));
}

#[test]
fn baseline_artifact_must_match_the_reviewed_manifest_hash() {
    let manifest = baseline_manifest_with_sha256("00bad");
    assert!(matches!(
        load_reviewed_baseline(&manifest, "windows-control-v3"),
        Err(BaselineError::ArtifactHashMismatch { .. })
    ));
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-perf report
cargo test -p rshare-perf compare
```

Expected: FAIL because the tool does not exist.

**Step 3: Implement one versioned artifact schema**

Implement:

```rust
#[derive(Serialize, Deserialize)]
pub struct PerfReport {
    pub schema_version: u16,
    pub scenario: String,
    pub scenario_parameters: BTreeMap<String, serde_json::Value>,
    pub scenario_config_sha256: String,
    pub random_seed: u64,
    pub commit: String,
    pub dirty: bool,
    pub binary_sha256: BTreeMap<String, String>,
    pub cargo_lock_sha256: String,
    pub build_profile: String,
    pub cargo_features: Vec<String>,
    pub rustflags: String,
    pub runner_id: String,
    pub runner_fingerprint: String,
    pub availability: Availability,
    pub toolchain: ToolchainFingerprint,
    pub hardware: HardwareFingerprint,
    pub warmup: DurationSpec,
    pub runs: Vec<PerfRun>,
    pub metrics: BTreeMap<String, MetricSummary>,
    pub queues: BTreeMap<String, QueueSummary>,
    pub errors: Vec<String>,
    pub rss: Option<RssSummary>,
    pub measurement_provenance: BTreeMap<String, MeasurementProvenance>,
    pub verdict: VerdictStatus,
}

pub struct MeasurementProvenance {
    pub method: String,
    pub uncertainty_us: Option<u64>,
    pub evidence_path: Option<String>,
    pub evidence_sha256: Option<String>,
    pub estimate_only: bool,
}

pub enum Availability {
    Available,
    Unsupported { reason: String },
    NotRun { reason: String },
}

pub enum VerdictStatus {
    Pass,
    Fail,
    Unstable,
    Unsupported,
    NotRun,
}
```

Every input report includes overwrite/gap/duplicate/out-of-order and reliable-overflow counters. Canonicalize scenario parameters recursively before hashing; sort/deduplicate feature lists and hash every participating daemon/desktop/perf binary by role. A run is complete only when the process exits successfully, the artifact validates against `perf/baselines/schema.json`, and every scenario-required metric/counter is present. Strict comparison requires exactly five complete runs from one immutable batch, matching schema, scenario/config hash, seed policy, build profile/features/RUSTFLAGS, binary/lockfile hash policy, runner fingerprint, and a reviewed baseline. It never drops a failed or outlying run and never fills a missing run selectively.

Define `perf/baselines/manifest.toml` with fail-closed entries:

```toml
[[baseline]]
id = "windows-control-v3-runner-a"
scenario = "quic-control-v3"
scenario_config_sha256 = "<sha256>"
runner_fingerprint = "<sha256>"
artifact_path = "perf/baselines/windows-control-v3-runner-a.json"
artifact_sha256 = "<sha256>"
source_commit = "<40-hex-commit>"
approval_ref = "github-pr:<owner>/<repo>#<merged-pr-number>"
```

The comparator resolves a baseline only through this manifest, verifies the canonical artifact SHA-256 before parsing, verifies all report fingerprints/configuration fields, and rejects placeholders or direct unmanifested artifact paths in strict mode. The fixed-runner workflow reads the manifest from the protected default branch and uses the GitHub API to verify that `approval_ref` is a merged PR with an `APPROVED` review by someone other than its author and whose reviewed diff contains the exact manifest entry and artifact hash; unavailable verification is a failure, not a warning. The PR number can be added to the candidate branch after the PR is opened and before review. `perf/baselines/README.md` documents the reviewed update procedure but is never itself authority.

If any metric in a five-run batch has CV >10%, archive that complete batch as unstable and rerun the entire five-run batch exactly once with the same configuration. Preserve both batches. Never cherry-pick, discard, or selectively replace individual runs. If the second complete batch remains unstable, fail as infrastructure instability rather than passing; if it stabilizes, report both and use the predeclared retry policy rather than choosing the more favorable batch.

**Step 4: Add harness entry points and the complete control matrix**

Add Criterion groups:

- `control/layout_resolve_target`
- `control/session_transition`
- `control/forward_mouse_remote_active`
- `codec/realtime_mouse_encode_decode`
- `codec/reliable_key_button_encode_decode`

Add Criterion as an explicit dev dependency and register both non-libtest harnesses:

```toml
# crates/rshare-core/Cargo.toml
[[bench]]
name = "control_hot_path"
harness = false

# crates/rshare-net/Cargo.toml
[[bench]]
name = "quic_loopback"
harness = false
```

Add `rshare-perf quic` scenarios:

- 125 Hz ×10 s;
- 500 Hz ×10 s;
- 1000 Hz ×60 s;
- 1000 Hz with diagnostics + status + audio + bulk;
- slow/fast peer isolation;
- recovery after an exact 100 ms stall.

The tool commands are:

```powershell
cargo run --release --locked -p rshare-perf -- quic --rate-hz 1000 --duration-secs 60 --load diagnostics,status,audio,bulk --output C:\tmp\rshare-perf\quic.json
cargo run --release --locked -p rshare-perf -- compare --baseline-id windows-control-v3-runner-a --candidate C:\tmp\rshare-perf\quic.json --budget perf\budgets\windows-fixed.toml
```

Real daemon IPC scenarios are added after the framed server seam exists in Task 14; do not benchmark a fake echo server or contend on hard-coded port 27435.

**Step 5: Compile and run the report tests**

Run:

```powershell
cargo test -p rshare-perf --locked report
cargo test -p rshare-perf --locked compare
cargo test -p rshare-core --locked
cargo test -p rshare-net --locked
cargo bench -p rshare-core --bench control_hot_path --no-run --locked
cargo bench -p rshare-net --bench quic_loopback --no-run --locked
cargo bench -p rshare-core --bench control_hot_path --locked -- --test
cargo bench -p rshare-net --bench quic_loopback --locked -- --test
```

Expected: all tests PASS and both benches compile.

**Step 6: Commit**

```powershell
git add Cargo.toml tools/rshare-perf perf crates/rshare-core/Cargo.toml crates/rshare-core/benches/control_hot_path.rs crates/rshare-net/Cargo.toml crates/rshare-net/benches/quic_loopback.rs docs/performance/README.md
git commit -m "test: add reproducible latency reports"
```

### Task 3: Version the Peer Protocol and Reject Incompatible Peers

**Files:**
- Create: `crates/rshare-core/src/protocol/handshake.rs`
- Modify: `crates/rshare-core/src/protocol/mod.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Create: `crates/rshare-net/src/handshake.rs`
- Modify: `crates/rshare-net/src/lib.rs`
- Modify: `crates/rshare-net/src/encryption.rs`
- Modify: `crates/rshare-net/src/transport.rs`
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/discovery.rs`
- Create: `crates/rshare-core/tests/protocol_v3_contract.rs`
- Create: `crates/rshare-net/tests/protocol_v3_handshake.rs`
- Create: `crates/rshare-net/tests/peer_identity.rs`

**Step 1: Write failing compatibility, ghost-peer, and identity tests**

Require:

```rust
#[test]
fn protocol_v3_is_the_only_accepted_peer_version() {
    assert_eq!(PROTOCOL_VERSION, 3);
    let hello = hello_message(DeviceId::new_v4(), "node".into(), "host".into());
    assert!(hello.advertises_required_v3_transport_capabilities());
}
```

Add:

- `inbound_v2_hello_is_rejected_before_registration`;
- `timed_out_hello_never_emits_connected`;
- `non_hello_first_message_never_enters_registry`;
- `outbound_surfaces_peer_rejection_reason`;
- `v3_peers_exchange_certificate_identity`;
- `v3_peer_without_client_certificate_is_rejected_after_hello`;
- `old_peer_can_complete_tls_then_receive_explicit_version_rejection`;
- `changed_fingerprint_never_enters_registry`;
- `discovery_surfaces_old_peer_as_incompatible_without_connecting`.
- `reconnect_old_disconnect_cannot_remove_new_generation`.

**Step 2: Run tests and verify failure**

Run:

```powershell
cargo test -p rshare-core --test protocol_v3_contract
cargo test -p rshare-net --test protocol_v3_handshake
cargo test -p rshare-net --test peer_identity
```

Expected: FAIL because the protocol is v1, invalid Hello can create a synthetic identity, and clients do not present certificates.

**Step 3: Add the versioned handshake contract**

Add serde-defaulted capabilities so an old broadcast remains parseable:

```rust
pub const PROTOCOL_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PeerTransportCapabilities {
    pub realtime_input_version: u16,
    pub reliable_input_version: u16,
    pub qos_lanes: bool,
    /// Optional capability. Zero means media is disabled/unsupported.
    pub separate_media_quic_version: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HandshakeRejectReason {
    ProtocolMismatch { required: u32, received: u32 },
    ApplicationMismatch,
    MissingCapabilities { missing: Vec<String> },
    IdentityUnavailable,
}

Message::HelloRejected {
    app_id: String,
    device_id: DeviceId,
    reason: HandshakeRejectReason,
}
```

Add `transport_capabilities` with `#[serde(default)]` to `Hello`/`HelloBack`. Protocol v3 requires only the realtime-input, reliable-input, and QoS control capabilities. `separate_media_quic_version` is optional and advertises `0` when disabled/unsupported; a media session is allowed only when both peers advertise the same nonzero version. This keeps Phase 2 independently disableable and rollback-safe. Discovery retains old peers with:

```rust
pub enum PeerProtocolCompatibility {
    Compatible,
    Incompatible { local: u32, remote: u32 },
}
```

and `connect_to` fails before QUIC for known incompatible discoveries. The UI can show the exact upgrade reason; the device is not silently hidden.

Only a successful `NegotiatedPeer` may receive a connection generation, enter the registry, or emit `Connected`. Delete the current fallback that generates a random `DeviceId` for invalid/missing/timed-out Hello.

**Step 4: Bind protocol v3 to mutual certificate identity**

In `rshare-core/src/protocol/handshake.rs`, add only the transport-neutral generation:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ControlConnectionId(Uuid);

impl ControlConnectionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
```

In `rshare-net/src/handshake.rs`, bind that generation to the existing network-layer certificate type:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerAuthContext {
    pub peer_id: DeviceId,
    pub certificate_fingerprint: PeerCertificateFingerprint,
    pub control_connection_id: ControlConnectionId,
}

pub enum ClientCertificatePolicy {
    OptionalForControlBootstrap,
    RequiredForMedia,
}
```

The v3 client uses its existing local certificate/key with `with_client_auth_cert`. The control server requests but does not make a certificate TLS-mandatory: this allows an old node to reach `HelloRejected`. Its custom verifier must still verify CertificateVerify signatures; never return an unconditional rustls assertion. After a valid v3 Hello, absence of a certificate is `IdentityUnavailable`.

Confirm claimed `DeviceId`, certificate fingerprint, TOFU trust decision, app id, protocol, and capabilities before registration. Do not require a new control ALPN yet, because a TLS-layer ALPN failure would prevent explicit old-peer rejection. Media uses a mandatory distinct ALPN in Task 24.

Keep one compatibility bootstrap stream with the existing bounded `u32 length + JSON` framing, a strict size limit, a short handshake timeout, and an allowlist containing only `Hello`, `HelloBack`, and `HelloRejected`. No input, diagnostics, file, audio, or media message is legal on this stream. Only after authentication and v3 capability negotiation may either side open the `RSQ3` lane-prefixed streams from Task 9. This guarantees a v2 peer can be parsed and explicitly rejected instead of failing on an unexpected lane preface.

**Step 5: Run protocol and network tests**

Run:

```powershell
cargo test -p rshare-core --test protocol_v3_contract
cargo test -p rshare-net --test protocol_v3_handshake
cargo test -p rshare-net --test peer_identity
cargo test -p rshare-net connection
cargo test -p rshare-net discovery
```

Expected: all tests PASS; old peers are visible as incompatible, receive an explicit rejection when applicable, and never enter the canonical registry.

**Step 6: Commit**

```powershell
git add crates/rshare-core/src/protocol/handshake.rs crates/rshare-core/src/protocol/mod.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/protocol_v3_contract.rs crates/rshare-net/src/handshake.rs crates/rshare-net/src/lib.rs crates/rshare-net/src/encryption.rs crates/rshare-net/src/transport.rs crates/rshare-net/src/connection.rs crates/rshare-net/src/discovery.rs crates/rshare-net/tests/protocol_v3_handshake.rs crates/rshare-net/tests/peer_identity.rs
git commit -m "feat: authenticate and reject peer protocol v3"
```

### Task 4: Define Epoch-Scoped Control Frames and Held-State Safety

**Files:**
- Create: `crates/rshare-core/src/input.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Create: `crates/rshare-core/tests/input_runtime_contract.rs`

**Step 1: Write the failing frame and ledger tests**

Cover:

- epoch increases and never reuses the old value;
- realtime sequence `10, 12, 11` accepts only 10 and 12;
- ownership transfer rejects the old peer;
- `ReleaseAll` lists every held key and mouse button exactly once;
- a reliable mouse-button frame carries a coordinate and realtime anchor.

Use this acceptance test:

```rust
#[test]
fn old_owner_and_late_sequence_are_rejected() {
    let owner_a = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let owner_b = authenticated_owner(DeviceId::new_v4(), ControlConnectionId::new());
    let mut gate = InputOwnershipGate::new(owner_a, SessionEpoch(7));
    assert_eq!(gate.accept_realtime(owner_a, SessionEpoch(7), 10), AcceptRealtime::Accepted);
    assert_eq!(gate.accept_realtime(owner_a, SessionEpoch(7), 12), AcceptRealtime::AcceptedWithGap(1));
    assert_eq!(gate.accept_realtime(owner_a, SessionEpoch(7), 11), AcceptRealtime::OutOfOrder);
    gate.transfer(owner_b, SessionEpoch(8));
    assert_eq!(gate.accept_realtime(owner_a, SessionEpoch(7), 13), AcceptRealtime::WrongOwnerOrEpoch);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core --test input_runtime_contract
```

Expected: FAIL because the shared input contracts do not exist.

**Step 3: Add the minimal wire/domain model**

Define:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SessionEpoch(pub u64);

pub const INPUT_PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RealtimeInputFrame {
    pub protocol_version: u16,
    pub session_epoch: SessionEpoch,
    pub sequence: u64,
    pub captured_at: MonotonicStamp,
    pub payload: RealtimeInputPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum RealtimeInputPayload {
    RelativeMouse { dx: i32, dy: i32 },
    AbsoluteAnchor { x: i32, y: i32 },
    GamepadAxes {
        gamepad_id: u8,
        left_stick_x: i16,
        left_stick_y: i16,
        right_stick_x: i16,
        right_stick_y: i16,
        left_trigger: u16,
        right_trigger: u16,
    },
    CursorVisual { x: i32, y: i32, visible: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReliableInputFrame {
    pub protocol_version: u16,
    pub session_epoch: SessionEpoch,
    pub sequence: u64,
    pub captured_at: MonotonicStamp,
    pub event: ReliableInputEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReliableInputEvent {
    Enter { target_display_id: String, x: i32, y: i32 },
    Leave,
    ReleaseAll { reason: ReleaseAllReason },
    Key { keycode: u32, state: KeyState },
    TextCommit { text: String },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
        x: i32,
        y: i32,
        realtime_anchor_sequence: u64,
    },
    Wheel { delta_x: i32, delta_y: i32 },
    GamepadConnected { info: GamepadDeviceInfo },
    GamepadDisconnected { gamepad_id: u8 },
    GamepadButton { gamepad_id: u8, button: GamepadButton, pressed: bool },
}
```

Add `AuthenticatedInputOwner { peer_id, control_connection_id }`, `InputOwnershipGate`, `PressedStateLedger`, `AcceptRealtime`, and `AcceptReliable`. `ControlConnectionId` is an unguessable, never-reused local connection generation defined with the handshake contract. The wire never carries or trusts an owner field: Task 10 derives the owner from the authenticated `PeerAuthContext`, and every gate operation checks both peer id and connection generation. `PressedStateLedger::release_all_events()` must return releases in deterministic order and clear the ledger only after the caller confirms successful injection.

**Step 4: Run core tests**

Run:

```powershell
cargo test -p rshare-core --test input_runtime_contract
cargo test -p rshare-core
```

Expected: all tests PASS.

**Step 5: Commit**

```powershell
git add crates/rshare-core/src/input.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/input_runtime_contract.rs
git commit -m "feat: add epoch scoped control frames"
```

### Task 5: Replace Unbounded Capture Queues with Semantic Bounded Ingress

**Files:**
- Create: `crates/rshare-input/src/ingress.rs`
- Modify: `crates/rshare-input/src/lib.rs`
- Modify: `crates/rshare-input/src/listener.rs`
- Modify: `crates/rshare-input/src/gamepad.rs`
- Create: `crates/rshare-input/tests/semantic_ingress.rs`

**Step 1: Write failing bounded/coalescing tests**

Tests must prove:

```rust
#[tokio::test]
async fn one_hundred_thousand_moves_use_one_pending_slot() {
    let (producer, mut consumer) = SemanticInputIngress::new(128);
    for x in 0..100_000 {
        producer.try_push(captured_mouse_move(x, x));
    }
    assert_eq!(producer.stats().pending_items, 1);
    assert_eq!(producer.stats().coalesced_motion, 99_999);
    assert_eq!(consumer.recv().await.unwrap().payload, pointer_absolute(99_999, 99_999));
}

#[tokio::test]
async fn coalescing_never_crosses_a_button_barrier() {
    let (producer, mut consumer) = SemanticInputIngress::new(8);
    producer.try_push(captured_mouse_move(1, 1));
    producer.try_push(captured_button_down_at(1, 1));
    producer.try_push(captured_mouse_move(2, 2));
    assert_eq!(consumer.recv().await.unwrap().payload, pointer_absolute(1, 1));
    assert!(matches!(
        consumer.recv().await.unwrap().payload,
        CapturedInputPayload::Discrete(InputEvent::MouseButton { .. })
    ));
    assert_eq!(consumer.recv().await.unwrap().payload, pointer_absolute(2, 2));
}

#[test]
fn discrete_overflow_is_explicit() {
    let (producer, _consumer) = SemanticInputIngress::new(1);
    producer.try_push(captured_key_down());
    assert_eq!(producer.try_push(captured_key_up()), PushOutcome::ReliableOverflow);
    assert!(matches!(
        producer.try_pop_fault(),
        Some(IngressFault::ReliableOverflow)
    ));
}

#[test]
fn replaceable_motion_cannot_turn_an_all_discrete_queue_into_a_reliable_fault() {
    let (producer, _consumer) = SemanticInputIngress::new(2);
    assert_eq!(producer.try_push(captured_key_down()), PushOutcome::Enqueued);
    assert_eq!(producer.try_push(captured_button_down()), PushOutcome::Enqueued);
    assert_eq!(
        producer.try_push(captured_mouse_move(3, 4)),
        PushOutcome::RealtimeDropped
    );
    assert_eq!(producer.try_pop_fault(), None);
}

#[test]
fn gamepad_adapter_splits_buttons_from_replaceable_axes() {
    let outputs = adapt_gamepad_snapshots(
        gamepad_state(false, 0),
        gamepad_state(true, 12_000),
    );
    assert!(matches!(
        outputs.as_slice(),
        [
            CapturedInputPayload::Discrete(InputEvent::GamepadButton { pressed: true, .. }),
            CapturedInputPayload::Continuous(GamepadAxes { left_stick_x: 12_000, .. }),
        ]
    ));
}
```

Also require `relative_motion_coalescing_accumulates_delta`, `mouse_button_snapshots_latest_pointer_coordinate`, and `reliable_overflow_uses_reserved_fault_slot`.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-input --test semantic_ingress
```

Expected: FAIL because `SemanticInputIngress` does not exist.

**Step 3: Implement a non-async producer and bounded consumer**

Move the daemon-local captured-event metadata into this transport-neutral input boundary:

```rust
pub enum PointerSample {
    Absolute { x: i32, y: i32 },
    Relative { dx: i32, dy: i32, observed_x: Option<i32>, observed_y: Option<i32> },
}

pub struct CaptureOrigin {
    pub source: CaptureSource,
    pub device_token: u64,
    pub instance_token: u64,
}

pub struct CapturedInput {
    pub captured_at: MonotonicStamp,
    pub ingress_enqueued_at: MonotonicStamp,
    pub origin: CaptureOrigin,
    pub payload: CapturedInputPayload,
}
```

Use numeric tokens on the hot path; format device ids/capture labels in diagnostics sampling. Use a short `std::sync::Mutex<VecDeque<CapturedInput>>` critical section plus `tokio::sync::Notify`; OS callbacks call only `try_push`.

```rust
pub enum PushOutcome {
    Enqueued,
    Coalesced,
    RealtimeReplaced,
    RealtimeDropped,
    ReliableOverflow,
    Closed,
}

pub struct SemanticInputProducer {
    shared: Arc<SharedIngress>,
}

pub struct SemanticInputConsumer {
    shared: Arc<SharedIngress>,
}

impl SemanticInputProducer {
    pub fn try_push(
        &self,
        mut item: CapturedInput,
    ) -> PushOutcome {
        item.ingress_enqueued_at = self.shared.clock.now();
        let mut state = self.shared.state.lock().expect("input ingress poisoned");
        if item.payload.is_replaceable_continuous_state()
            && state.queue.back().is_some_and(|back| {
                back.payload.same_replaceable_class(&item.payload)
            })
        {
            state.queue.back_mut().expect("checked").payload
                .coalesce_from(item.payload);
            state.coalesced_motion += 1;
            return PushOutcome::Coalesced;
        }
        if state.queue.len() == state.capacity {
            if let Some(index) = state.oldest_replaceable_index() {
                state.queue.remove(index);
                state.queue.push_back(item);
                return PushOutcome::RealtimeReplaced;
            }
            if item.payload.is_replaceable_continuous_state() {
                state.dropped_realtime += 1;
                return PushOutcome::RealtimeDropped;
            }
            state.reliable_overflow_latched = true;
            self.shared.ready.notify_one();
            return PushOutcome::ReliableOverflow;
        }
        state.queue.push_back(item);
        drop(state);
        self.shared.ready.notify_one();
        PushOutcome::Enqueued
    }
}
```

Classify only mouse motion and gamepad axis snapshots as replaceable. Adjacent relative motion accumulates deltas; adjacent absolute motion keeps only the latest position. A key/button/wheel/text/session item is always a barrier and snapshots the latest pointer coordinate. Adapt the existing full-state `GilrsGamepadListener` by diffing consecutive snapshots: every press/release becomes an ordered reliable `GamepadButton` barrier, while only axis/trigger changes become replaceable `GamepadAxes`; coalescing a state snapshot must never erase a button transition. A discrete event may evict an older replaceable item to make room. If the queue contains only discrete events, an incoming replaceable item is dropped and counted without fault; only an incoming discrete event latches a reserved `IngressFault::ReliableOverflow` outside the ordinary queue. The router will suspend and use the reserved emergency `ReleaseAll` path. Never block the OS callback and never silently discard a discrete item.

Adapt `InputEventChannel` and gamepad callers to the producer API but do not wire the daemon overflow policy until Task 12.

**Step 4: Run input tests**

Run:

```powershell
cargo test -p rshare-input --test semantic_ingress
cargo test -p rshare-input
```

Expected: all tests PASS, including 100,000-move boundedness.

**Step 5: Commit**

```powershell
git add crates/rshare-input/src/ingress.rs crates/rshare-input/src/lib.rs crates/rshare-input/src/listener.rs crates/rshare-input/src/gamepad.rs crates/rshare-input/tests/semantic_ingress.rs
git commit -m "feat: add semantic bounded input ingress"
```

### Task 6: Extract a Single Pure InputRouter Core

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rshare-core/Cargo.toml`
- Create: `crates/rshare-core/src/input_router.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/src/layout.rs`
- Test: `crates/rshare-core/src/input_router.rs`
- Test: `crates/rshare-core/tests/layout_graph_contract.rs`

**Step 1: Write failing routing tests**

Add deterministic tests for:

- entering left/right/top/bottom using real virtual-desktop bounds, including negative coordinates;
- 1080p→1440p and 4K→1080p target projection;
- one `Enter` barrier before the first new-epoch realtime frame;
- relative motion after entry;
- quick return and disconnect emit `ReleaseAll` before local ownership resumes;
- layout/connection changes update a cached route without rebuilding peer sets per input.

Representative test:

```rust
#[test]
fn entry_uses_real_bounds_then_routes_relative_motion() {
    let local = DeviceId::new_v4();
    let remote = DeviceId::new_v4();
    let mut router = fixture_router(
        local,
        DesktopBounds { x: -1920, y: 0, width: 3840, height: 2160 },
        linked_right(remote, DisplayGeometry::new(0, 0, 2560, 1440, 1.25)),
    );
    let entered = router.handle(RouterCommand::Input(
        RouterInput::absolute_move(1919, 700, 10),
    ));
    assert!(matches!(
        entered.first(),
        Some(RouterOutput::SendReliable {
            target,
            frame: ReliableInputFrame { event: ReliableInputEvent::Enter { .. }, .. },
        }) if *target == remote
    ));
    let moved = router.handle(RouterCommand::Input(
        RouterInput::relative_move(7, -3, 11),
    ));
    assert!(matches!(moved.as_slice(), [
        RouterOutput::SendRealtime {
            target,
            frame: RealtimeInputFrame {
                payload: RealtimeInputPayload::RelativeMouse { dx: 7, dy: -3 },
                ..
            },
        } if *target == remote
    ]));
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core input_router::tests
```

Expected: FAIL because the router types do not exist.

**Step 3: Implement the pure single-owner router**

Implement:

```rust
pub enum RouterCommand {
    Input(RouterInput),
    LayoutChanged(LayoutGraph),
    ConnectivityChanged { peer: DeviceId, connected: bool },
    BackendDegraded,
    LeaseExpired,
    Shutdown,
}

pub enum RouterOutput {
    SendRealtime { target: DeviceId, frame: RealtimeInputFrame },
    SendReliable { target: DeviceId, frame: ReliableInputFrame },
    EmergencyReleaseAll { target: DeviceId, frame: ReliableInputFrame },
    LocalSessionChanged(ControlSessionState),
    SuppressLocalShortcuts(bool),
    Metric(RouterMetric),
}

pub struct InputRouter {
    local_id: DeviceId,
    epoch: SessionEpoch,
    realtime_sequence: u64,
    reliable_sequence: u64,
    session: CaptureSessionStateMachine,
    routes: RouteCache,
    geometry: DesktopGeometry,
    pressed: PressedStateLedger,
}

impl InputRouter {
    pub fn handle(
        &mut self,
        command: RouterCommand,
    ) -> smallvec::SmallVec<[RouterOutput; 4]>;
}
```

Every network-bound output carries the target captured before a session transition mutates or clears router state; this is especially required for quick-return, disconnect, overflow, and backend-fault `ReleaseAll`. Add `smallvec = "1.15"` to workspace/core dependencies. Use fixed arrays/bitmasks for edge checks and `SmallVec<[RouterOutput; 4]>` for the uncommon multi-action case. Extend `DisplayNode` with serde-defaulted scale/DPI fields and add `VirtualDesktopGeometry`/`PixelRect` construction from `LocalDisplayState`. Extend `LayoutGraph` with an indexed four-direction `RouteCache` built only on layout/connection changes; normal motion must not allocate, enumerate displays, or build a `HashSet`.

Do not depend on Tokio, `rshare-input`, daemon state, or network types in this module.

**Step 4: Run core correctness tests**

Run:

```powershell
cargo test -p rshare-core input_router
cargo test -p rshare-core --test layout_graph_contract
cargo test -p rshare-core --test session_state_machine
```

Expected: all tests PASS.

**Step 5: Commit**

```powershell
git add Cargo.toml crates/rshare-core/Cargo.toml crates/rshare-core/src/input_router.rs crates/rshare-core/src/lib.rs crates/rshare-core/src/layout.rs crates/rshare-core/tests/layout_graph_contract.rs
git commit -m "refactor: extract single input router core"
```

### Task 7: Preserve Epoch, Sequence, and Timestamp in Realtime Codec

**Files:**
- Modify: `crates/rshare-net/src/codec.rs`
- Test: `crates/rshare-net/src/codec.rs`

**Step 1: Write failing codec tests**

Replace tests that decode realtime data directly into legacy `Message` with:

```rust
#[test]
fn realtime_round_trip_preserves_ordering_metadata() {
    let frame = RealtimeInputFrame {
        protocol_version: INPUT_PROTOCOL_VERSION,
        session_epoch: SessionEpoch(42),
        sequence: 9,
        captured_at: fixture_stamp(123_456),
        payload: RealtimeInputPayload::RelativeMouse { dx: 7, dy: -4 },
    };
    let encoded = RealtimeInputCodec::encode(&frame).unwrap();
    assert_eq!(RealtimeInputCodec::decode(&encoded).unwrap(), frame);
}

#[test]
fn realtime_decode_rejects_wrong_version_and_trailing_bytes() {
    // Mutate version and length independently; both must fail closed.
}
```

Add a receiver-filter test proving `10, 12, 11` decodes but only 10 and 12 are accepted by `InputOwnershipGate`.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-net realtime_round_trip_preserves_ordering_metadata
```

Expected: FAIL because `decode_message` discards the metadata.

**Step 3: Replace the legacy realtime mapping**

Use this binary header:

```text
u16 input_protocol_version
u8  kind
u16 payload_length
u64 session_epoch
u64 sequence
u64 captured_clock_domain
u64 captured_at_us
N   payload
```

Expose only:

```rust
impl RealtimeInputCodec {
    pub fn encode(frame: &RealtimeInputFrame) -> anyhow::Result<bytes::Bytes>;
    pub fn decode(data: &[u8]) -> anyhow::Result<RealtimeInputFrame>;
}
```

Use fixed-width payloads for mouse motion/anchor and a bounded payload for gamepad/cursor state. Remove reliable-stream fallback for realtime frames: datagram congestion returns `RealtimeSendOutcome::DroppedLatest`, increments a counter, and allows the next frame to replace it.

**Step 4: Run network codec tests**

Run:

```powershell
cargo test -p rshare-net codec::tests
cargo test -p rshare-net
```

Expected: all tests PASS and no test expects a stale realtime frame to fall back to reliable delivery.

**Step 5: Commit**

```powershell
git add crates/rshare-net/src/codec.rs
git commit -m "feat: retain realtime input ordering metadata"
```

### Task 8: Add Compact Dedicated Reliable-Input Codec

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/rshare-net/src/codec.rs`
- Test: `crates/rshare-net/src/codec.rs`

**Step 1: Write failing reliable-input codec tests**

Round-trip every `ReliableInputEvent` variant and enforce:

- maximum frame length;
- unknown protocol/tag rejection;
- exact consumption with no trailing bytes;
- deterministic encoding for the same input;
- button anchor coordinates and sequence survive round-trip.
- a valid outer prefix with an invalid body `frame.protocol_version` is rejected.

```rust
#[test]
fn mouse_button_anchor_round_trips() {
    let frame = reliable_button_fixture(SessionEpoch(4), 22, 800, 450, 19);
    let bytes = ReliableInputCodec::encode(&frame).unwrap();
    assert_eq!(ReliableInputCodec::decode(&bytes).unwrap(), frame);
    assert!(bytes.len() < 96);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-net mouse_button_anchor_round_trips
```

Expected: FAIL because `ReliableInputCodec` does not exist.

**Step 3: Implement the codec**

Enable bincode serde support at the workspace level:

```toml
bincode = { version = "2.0", features = ["serde"] }
```

Use a two-byte input protocol version and a deterministic fixed-integer bincode configuration with a hard size limit:

```rust
pub const MAX_RELIABLE_INPUT_FRAME: usize = 4 * 1024;

pub struct ReliableInputCodec;

impl ReliableInputCodec {
    pub fn encode(frame: &ReliableInputFrame) -> anyhow::Result<bytes::Bytes> {
        anyhow::ensure!(
            frame.protocol_version == INPUT_PROTOCOL_VERSION,
            "unsupported reliable input body version"
        );
        let config = bincode::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding();
        let body = bincode::serde::encode_to_vec(frame, config)?;
        anyhow::ensure!(body.len() <= MAX_RELIABLE_INPUT_FRAME, "reliable input too large");
        let mut encoded = BytesMut::with_capacity(2 + body.len());
        encoded.put_u16(INPUT_PROTOCOL_VERSION);
        encoded.extend_from_slice(&body);
        Ok(encoded.freeze())
    }

    pub fn decode(data: &[u8]) -> anyhow::Result<ReliableInputFrame> {
        anyhow::ensure!(data.len() >= 2 && data.len() <= MAX_RELIABLE_INPUT_FRAME + 2,
            "invalid reliable input length");
        anyhow::ensure!(
            u16::from_be_bytes([data[0], data[1]]) == INPUT_PROTOCOL_VERSION,
            "unsupported reliable input version"
        );
        let config = bincode::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding();
        let (frame, consumed) = bincode::serde::decode_from_slice(&data[2..], config)?;
        anyhow::ensure!(consumed == data.len() - 2, "trailing reliable input bytes");
        anyhow::ensure!(
            frame.protocol_version == INPUT_PROTOCOL_VERSION,
            "reliable input prefix/body version mismatch"
        );
        Ok(frame)
    }
}
```

Enforce the size before decoding so malformed length cannot cause an oversized allocation. Bound `TextCommit` separately (for example, 4 KiB UTF-8) even though the outer frame is bounded.

**Step 4: Run codec and workspace-contract tests**

Run:

```powershell
cargo test -p rshare-net codec::tests
cargo test -p rshare-core
```

Expected: all tests PASS.

**Step 5: Commit**

```powershell
git add Cargo.toml crates/rshare-net/src/codec.rs
git commit -m "feat: add dedicated reliable input codec"
```

### Task 9: Replace Nested Transport Locks with Cloneable QoS Handles

**Files:**
- Create: `crates/rshare-net/src/qos.rs`
- Modify: `crates/rshare-net/src/lib.rs`
- Modify: `crates/rshare-net/src/transport.rs`
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/network_manager.rs`
- Test: `crates/rshare-net/src/qos.rs`
- Test: `crates/rshare-net/src/transport.rs`
- Test: `crates/rshare-net/src/network_manager.rs`

**Step 1: Write failing lane-isolation and lock-scope tests**

Create a fake `LaneWriter` whose bulk lane never completes and whose reliable-input lane records writes. Prove:

```rust
#[tokio::test]
async fn blocked_bulk_lane_does_not_delay_reliable_input() {
    let (handle, probe) = qos_fixture_with_blocked_bulk();
    let bulk = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .send_bulk(BulkFrame::test_payload(vec![0; 1_000_000]))
                .await
        }
    });
    handle.try_send_reliable_input(reliable_key_down()).unwrap();
    tokio::time::timeout(Duration::from_millis(20), probe.next_reliable())
        .await
        .expect("reliable input must bypass blocked bulk");
    bulk.abort();
}

#[tokio::test]
async fn slow_peer_does_not_block_fast_peer_or_registry_read() {
    let registry = fixture_registry_with_slow_and_fast_peers();
    let slow_send = tokio::spawn({
        let handle = registry.peer(&SLOW).unwrap().transport;
        async move { handle.send_control(large_control()).await }
    });
    let fast = registry.peer(&FAST).unwrap().transport;
    fast.try_send_reliable_input(reliable_key_down()).unwrap();
    assert_eq!(registry.snapshot().len(), 2);
    slow_send.abort();
}
```

Add a realtime test that fills all other lanes and still calls `quinn::Connection::send_datagram` without entering a FIFO writer.

Also prove terminal and connection-generation behavior:

- block an actual reliable QUIC stream write with flow control, enqueue same-epoch key-downs, then close the epoch; the independent emergency stream delivers `ReleaseAll`, and no old-epoch frame is accepted after it;
- if the one reserved emergency slot is already occupied, the transport closes the control connection and the receiver's connection-loss path releases locally;
- after reconnect, a delayed disconnect from the old `ControlConnectionId` cannot remove the new registry entry.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-net qos::tests -- --nocapture
cargo test -p rshare-net slow_peer_does_not_block_fast_peer_or_registry_read
```

Expected: FAIL because all traffic uses the single `send_channel` and connection-table lock.

**Step 3: Add cloneable per-peer handles**

Implement:

```rust
#[derive(Clone)]
pub struct PeerTransportHandle {
    inner: Arc<PeerTransportInner>,
}

impl PeerTransportHandle {
    pub fn try_send_realtime(
        &self,
        frame: RealtimeInputFrame,
    ) -> Result<RealtimeSendOutcome, TransportSendError>;

    pub fn try_send_reliable_input(
        &self,
        frame: ReliableInputFrame,
    ) -> Result<(), TransportSendError>;

    pub fn try_send_emergency(
        &self,
        frame: ReliableInputFrame,
    ) -> Result<(), TransportSendError>;

    pub async fn send_control(&self, frame: ControlFrame)
        -> Result<(), TransportSendError>;

    pub async fn send_bulk(&self, payload: BulkFrame)
        -> Result<(), TransportSendError>;

    pub fn try_send_telemetry(&self, frame: TelemetryFrame)
        -> Result<(), TransportSendError>;
}
```

Use:

- direct QUIC datagram submission for realtime;
- a dedicated persistent reliable-input stream with a bounded 256-frame channel;
- a reserved one-frame emergency slot driving its own highest-priority unidirectional stream, independent from a blocked reliable `write_all`;
- independent control, bulk, and sampled-telemetry streams/writers;
- lane preface `RSQ3` plus a lane discriminator at the start of every accepted unidirectional stream;
- Quinn stream priority: reliable input highest, control next, telemetry/bulk lowest;
- an accept loop that spawns a reader per accepted unidirectional stream instead of blocking forever on the first stream.

Replace `ConnectionPool<HashMap<DeviceId, QuicConnection>>` with:

```rust
#[derive(Clone)]
pub struct RegisteredPeer {
    pub auth: Arc<PeerAuthContext>,
    pub transport: PeerTransportHandle,
}

pub struct ConnectionRegistry {
    peers: std::sync::RwLock<HashMap<DeviceId, RegisteredPeer>>,
}

impl ConnectionRegistry {
    pub fn peer(&self, id: &DeviceId) -> Option<RegisteredPeer> {
        self.peers.read().expect("connection registry poisoned").get(id).cloned()
    }

    pub fn snapshot(&self) -> Vec<(DeviceId, RegisteredPeer)> {
        self.peers.read().expect("connection registry poisoned")
            .iter()
            .map(|(id, peer)| (*id, peer.clone()))
            .collect()
    }

    pub fn remove_if_generation(
        &self,
        id: DeviceId,
        connection_id: ControlConnectionId,
    ) -> Option<RegisteredPeer>;
}
```

`ControlFrame`, `TelemetryFrame`, and `BulkFrame` are closed lane-specific envelopes in `qos.rs`, not type aliases for `Message`. Implement one exhaustive `TryFrom<Message>` classifier so audio, file, diagnostics, or a future variant cannot accidentally enter the control lane. The compatibility handshake remains on the bounded bootstrap stream from Task 3; `RSQ3` is required only after a peer becomes `NegotiatedPeer`.

The input hot path uses only `try_send_*`; a full reliable lane returns explicit backpressure so the router can suspend. On overflow/fault, atomically tombstone the current epoch before admitting any later normal frame, purge queued frames from that epoch, reset/stop its ordinary reliable stream, and send `ReleaseAll` on the independent emergency stream as a terminal barrier. Receivers tombstone that `(authenticated connection generation, epoch)` before local release and reject every later realtime/reliable frame for it, including frames arriving from an already-open stream. Sequence gap/duplicate on the reliable lane fails closed. If the emergency slot is full or the emergency write cannot be started, close the control connection immediately; connection loss and the short local lease trigger local release. A new epoch may proceed only through a new `Enter`.

Every general send/broadcast clones registered peer handles while locked, drops the lock, then awaits. Disconnect uses `remove_if_generation`; an old connection's delayed cleanup cannot remove a replacement. Broadcast peers independently with `FuturesUnordered`; one failure must not delay another.

**Step 4: Run transport and manager tests**

Run:

```powershell
cargo test -p rshare-net qos::tests
cargo test -p rshare-net transport::tests
cargo test -p rshare-net network_manager::tests
```

Expected: all tests PASS; the slow-peer tests complete inside their timeouts.

**Step 5: Commit**

```powershell
git add crates/rshare-net/src/qos.rs crates/rshare-net/src/lib.rs crates/rshare-net/src/transport.rs crates/rshare-net/src/connection.rs crates/rshare-net/src/network_manager.rs
git commit -m "refactor: split peer transport into qos lanes"
```

### Task 10: Split Inbound Realtime, Reliable Input, and General Events

**Files:**
- Modify: `crates/rshare-net/src/lib.rs`
- Modify: `crates/rshare-net/src/transport.rs`
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/network_manager.rs`
- Test: `crates/rshare-net/src/transport.rs`
- Test: `crates/rshare-net/src/connection.rs`

**Step 1: Write failing inbound-isolation tests**

Prove:

- a saturated telemetry/control receiver does not stop realtime datagram draining;
- realtime sequence metadata reaches the daemon-facing event;
- reliable input has its own bounded receiver and preserves order;
- a malformed lane discriminator closes only that stream and reports a protocol error.

```rust
#[tokio::test]
async fn control_backpressure_does_not_block_realtime_receive() {
    let mut fixture = connected_fixture_with_control_receiver_blocked().await;
    for sequence in 1..=100 {
        fixture.sender.try_send_realtime(mouse_frame(sequence)).unwrap();
    }
    let latest = tokio::time::timeout(
        Duration::from_millis(100),
        fixture.receiver.realtime_rx.recv(),
    ).await.unwrap().unwrap();
    assert_eq!(latest.sequence, 100);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-net control_backpressure_does_not_block_realtime_receive
```

Expected: FAIL because all inbound messages merge into one `mpsc<Message>`.

**Step 3: Add typed inbound channels**

Expose:

```rust
pub struct PeerInbound {
    pub auth: Arc<PeerAuthContext>,
    pub realtime_rx: mpsc::Receiver<RealtimeInputFrame>,
    pub reliable_input_rx: mpsc::Receiver<ReliableInputFrame>,
    pub control_rx: mpsc::Receiver<ControlFrame>,
    pub telemetry_rx: mpsc::Receiver<TelemetryFrame>,
    pub bulk_rx: mpsc::Receiver<BulkFrame>,
}

pub struct NetworkReceivers {
    /// Yields one isolated receiver set per authenticated connection generation.
    pub authenticated_peers: mpsc::Receiver<PeerInbound>,
    pub events: mpsc::Receiver<NetworkEvent>,
}

pub enum NetworkEvent {
    DeviceFound(DiscoveredDevice),
    DeviceConnected(PeerAuthContext),
    DeviceDisconnected {
        peer_id: DeviceId,
        control_connection_id: ControlConnectionId,
    },
    ControlReceived {
        auth: Arc<PeerAuthContext>,
        frame: ControlFrame,
    },
    ConnectionError {
        peer_id: Option<DeviceId>,
        control_connection_id: Option<ControlConnectionId>,
        error: String,
    },
}
```

Realtime ingress uses latest-value/coalescing semantics independently per authenticated peer connection; a noisy non-owner can never overwrite the owner's slot. Reliable input uses a bounded ordered queue. If that receiver is full, a reliable sequence gap/duplicate occurs, or a terminal epoch receives another frame, fail closed: tombstone the authenticated epoch, stop that lane/connection, and request local release rather than dropping a reliable event. Neither input lane appears in `NetworkEvent` or traverses the general 100-item manager FIFO. Task 12 consumes `NetworkReceivers.authenticated_peers`, derives `AuthenticatedInputOwner` only from `PeerInbound.auth`, and spawns dedicated forwarders into the daemon-owned injection runtime.

Decode a closed lane-specific envelope and reject a frame whose payload does not belong to that lane. The exhaustive `Message` classifier from Task 9 is the sole compatibility adapter for legacy general messages; adding a new `Message` variant must fail compilation until its lane is deliberately chosen.

**Step 4: Run all network tests**

Run:

```powershell
cargo test -p rshare-net
```

Expected: all tests PASS; no realtime decoder returns a legacy `Message`.

**Step 5: Commit**

```powershell
git add crates/rshare-net/src/lib.rs crates/rshare-net/src/transport.rs crates/rshare-net/src/connection.rs crates/rshare-net/src/network_manager.rs
git commit -m "refactor: isolate inbound input lanes"
```

### Task 11: Move Remote Injection to a Dedicated Ordered Actor

**Files:**
- Create: `crates/rshare-input/src/injection_actor.rs`
- Modify: `crates/rshare-input/src/lib.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Modify: `crates/rshare-input/src/emulator.rs`
- Modify: `crates/rshare-platform/src/windows.rs`
- Create: `crates/rshare-input/tests/injection_actor.rs`
- Test: `crates/rshare-input/src/emulator.rs`

**Step 1: Write failing ownership, anchor, and timing tests**

Use a mock backend recording calls and timestamps. Require:

- only current owner/current epoch can inject;
- realtime `10, 12, 11` injects 10 and 12;
- a button anchored to missing motion 11 first moves to its coordinate, then clicks;
- when realtime S+1 is already pending before a button anchored to S arrives, inject the button's embedded anchor and click before applying S+1;
- lease timeout/backend failure/connection loss invokes local `ReleaseAll`;
- a terminal `ReleaseAll` purges every queued same-epoch realtime/reliable item and rejects late arrivals;
- a reliable sequence gap/duplicate closes the epoch and releases locally;
- default realtime backend delay is zero;
- an intentionally slow backend does not block the Tokio network receiver.

```rust
#[test]
fn missing_motion_anchor_repairs_pointer_before_click() {
    let (actor, calls) = injection_fixture();
    actor.accept_realtime(PEER, mouse_frame(10, 100, 100)).unwrap();
    actor.accept_reliable(PEER, button_frame(2, 11, 240, 180)).unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [Call::Move(100, 100), Call::Move(240, 180), Call::ButtonDown(Left)]
    );
}

#[test]
fn future_realtime_waits_behind_earlier_reliable_anchor() {
    let (actor, calls) = injection_fixture();
    actor.accept_realtime(AUTH, mouse_frame(12, 300, 200)).unwrap();
    actor.accept_reliable(AUTH, button_frame(2, 11, 240, 180)).unwrap();
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        [
            Call::Move(240, 180),
            Call::ButtonDown(Left),
            Call::Move(300, 200),
        ]
    );
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-input --test injection_actor
cargo test -p rshare-input emulator_config_defaults
```

Expected: FAIL because injection is synchronous under a Tokio mutex and default delay is 1 ms.

**Step 3: Implement the dedicated actor**

Create one blocking OS thread that exclusively owns `Box<dyn InjectBackend>`. Its bounded semantic queue coalesces only adjacent realtime pointer/gamepad state and never crosses reliable barriers.

```rust
#[derive(Clone)]
pub struct InputInjectionHandle {
    queue: Arc<InjectionQueue>,
}

impl InputInjectionHandle {
    pub fn begin_session(
        &self,
        owner: AuthenticatedInputOwner,
        epoch: SessionEpoch,
        lease_duration: Duration,
    ) -> Result<(), InjectionQueueFull>;
    pub fn submit_realtime(
        &self,
        from: AuthenticatedInputOwner,
        frame: RealtimeInputFrame,
    ) -> RealtimeSubmitResult;
    pub fn try_submit_reliable(
        &self,
        from: AuthenticatedInputOwner,
        frame: ReliableInputFrame,
    ) -> Result<(), InjectionQueueFull>;
    pub fn request_release_all(&self, reason: ReleaseAllReason);
    pub fn shutdown(&self) -> Result<(), InjectionShutdownError>;
}
```

The queue has a bounded reliable FIFO, a latest realtime slot, and a reserved release/fault slot, but dequeue is sequence-aware rather than lane-priority-only. For a reliable barrier anchored to realtime sequence S, hold a pending realtime S+1, apply the reliable event's embedded coordinate/anchor first, then apply S+1. An epoch-close fault atomically tombstones the authenticated `(peer, control_connection_id, epoch)`, purges its ordinary FIFO/latest slot, performs local `ReleaseAll` as a terminal barrier, and rejects every late same-epoch frame. Do not inject queued key-downs after that barrier.

The worker owns `InputOwnershipGate` and `PressedStateLedger`. It derives ownership from `PeerAuthContext`, never from a wire-declared owner, and binds ownership to `control_connection_id`. It accepts a lease duration, computes its deadline with the receiver's local monotonic clock, validates before mapping to `InputEvent`, records injection-start/completion stamps, updates the ledger only after successful backend calls, and runs fail-safe releases locally even when the network is unavailable. It runs on a named `std::thread`; tests assert backend calls occur on that thread.

Extend `InjectBackend` with `inject_relative_pointer(dx, dy)`. Windows `SendInput` uses only `MOUSEEVENTF_MOVE`; Virtual HID calls its existing relative report and does not query the current cursor. Absolute anchors keep the current absolute path.

Set `EmulatorConfig::default().event_delay = Duration::ZERO`. If a non-realtime compatibility mode needs delay, require explicit construction; remove the unused `immediate` flag or make it the single source of truth.

**Step 4: Run injection and daemon tests**

Run:

```powershell
cargo test -p rshare-input emulator
cargo test -p rshare-input --test injection_actor
cargo test -p rshare-platform windows_relative_mouse
```

Expected: all tests PASS, no injection implementation contains an unconditional `sleep`.

**Step 5: Commit**

```powershell
git add crates/rshare-input/src/injection_actor.rs crates/rshare-input/src/lib.rs crates/rshare-input/src/backend.rs crates/rshare-input/src/emulator.rs crates/rshare-input/tests/injection_actor.rs crates/rshare-platform/src/windows.rs
git commit -m "refactor: move input injection off async workers"
```

### Task 12: Wire All Capture Paths Through One InputRouter Actor

**Files:**
- Modify: `apps/rshare-daemon/Cargo.toml`
- Create: `apps/rshare-daemon/src/lib.rs`
- Create: `apps/rshare-daemon/src/input_runtime.rs`
- Create: `apps/rshare-daemon/src/input_state.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-daemon/src/endpoint_runtime.rs`
- Modify: `apps/rshare-daemon/src/mobile_gateway.rs`
- Modify: `crates/rshare-core/src/protocol/mod.rs`
- Modify: `crates/rshare-input/src/listener.rs`
- Modify: `crates/rshare-input/src/gamepad.rs`
- Create: `apps/rshare-daemon/tests/input_pipeline_integration.rs`
- Test: `apps/rshare-daemon/src/main.rs`

**Step 1: Write failing actor integration tests**

Build fake ingress, transport, state sink, and shutdown handles. Prove:

- Windows hook, filter, portable, and gamepad inputs enter the same router in sequence;
- router output enqueues control before any diagnostic work;
- a 100 ms blocked transport overwrites mouse motion but preserves the next key;
- discrete-ingress overflow suspends the session and sends emergency `ReleaseAll`;
- connection loss and daemon shutdown release held state;
- every `RouterOutput` is sent to its embedded target, including release after router state is already suspended;
- input ownership is derived from the authenticated peer plus `ControlConnectionId`, and an old connection generation cannot inject after reconnect;
- state/layout snapshots update the router cache without taking daemon state per input event.

```rust
#[tokio::test]
async fn blocked_transport_keeps_latest_motion_and_reliable_key() {
    let fixture = InputRuntimeFixture::blocked();
    for x in 0..100_000 {
        assert!(matches!(
            fixture.capture.try_push(mouse_move(x, x)),
            PushOutcome::Enqueued
                | PushOutcome::Coalesced
                | PushOutcome::RealtimeReplaced
                | PushOutcome::RealtimeDropped
        ));
    }
    assert_eq!(
        fixture.capture.try_push(key_down(0x41)),
        PushOutcome::Enqueued
    );
    fixture.unblock_transport();
    assert_eq!(fixture.sent_realtime().last().unwrap().x(), 99_999);
    assert_eq!(fixture.sent_reliable(), vec![reliable_key_down(0x41)]);
    assert!(fixture.max_pending() <= fixture.configured_capacity());
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-daemon input_runtime::tests -- --nocapture
```

Expected: FAIL because two forwarding loops own independent routers and await diagnostics/network sends.

**Step 3: Add the actor and replace both forwarding loops**

Implement:

```rust
pub struct InputRuntime {
    ingress: SemanticInputConsumer,
    router: InputRouter,
    transports: Arc<ConnectionRegistry>,
    state: InputStatePublisher,
    metrics: Arc<ControlMetrics>,
    injection: InputInjectionHandle,
}
```

Define the minimal Task-12-owned seam in `input_state.rs`; do not reference Task 16 types:

```rust
pub enum InputUiMutation {
    KeyButton(InputDiscreteProjection),
    Session(ControlSessionState),
}

#[derive(Clone)]
pub struct InputStatePublisher {
    authoritative: watch::Sender<Arc<InputReliableUiProjection>>,
    reliable_tx: mpsc::Sender<InputUiMutation>,
    pointer_tx: watch::Sender<Option<InputPointerProjection>>,
    dirty: Arc<DirtyProjectionNotifier>,
}

pub struct InputPointerProjection {
    pub session_epoch: SessionEpoch,
    pub x: i32,
    pub y: i32,
}

pub struct InputDiscreteProjection {
    pub session_epoch: SessionEpoch,
    pub pressed_keys: Vec<u32>,
    pub pressed_buttons: Vec<MouseButton>,
}

pub struct InputReliableUiProjection {
    pub discrete: InputDiscreteProjection,
    pub session: ControlSessionState,
}

pub struct DirtyProjectionNotifier {
    dirty: AtomicBool,
    wake: Notify,
}

pub struct InputStateFeeds {
    pub reliable_rx: mpsc::Receiver<InputUiMutation>,
    pub authoritative_rx: watch::Receiver<Arc<InputReliableUiProjection>>,
    pub pointer_rx: watch::Receiver<Option<InputPointerProjection>>,
    pub dirty: Arc<DirtyProjectionNotifier>,
}

pub struct ControlMetrics {
    // Fixed atomic counters/histogram handoff only.
}

pub struct ControlMetricSnapshot {
    pub captured: u64,
    pub routed: u64,
    pub realtime_replaced: u64,
    pub realtime_dropped: u64,
    pub reliable_overflow: u64,
}
```

`input_state_channel(capacity)` returns `(InputStatePublisher, InputStateFeeds)`. `publish_pointer` uses `watch::send_replace`. A reliable input/session publication first replaces the small authoritative projection, then `try_send`s `InputUiMutation`; failure signals the reserved dirty notifier. These Task-12-local projection types depend only on already-existing core input/session types—not Task 15 UI types. Thus Task 12 compiles and preserves recoverable truth without knowing `UiChange`, `StateChange`, or `StateAggregator`. Until Task 16, a low-priority legacy adapter consumes `InputStateFeeds` and updates the existing UI snapshot off the input hot path; Task 16 replaces that adapter. `ControlMetrics` exposes nonblocking counter updates plus a read-only `snapshot()` used by Task 13. Tasks 15-16 map this seam into the full UI protocol/projection.

Processing order for each item:

1. stamp router dequeue;
2. run pure router synchronously;
3. use the target embedded in each `RouterOutput`, clone that exact generation's peer handle, and submit through `try_send_*`;
4. update `ControlMetrics` and publish through the Task-12-owned `InputStatePublisher`; pointer is latest-value, while reliable UI state first updates the authoritative projection and can trigger a dirty rebuild;
5. never await diagnostics or a global daemon lock.

Portable hooks, Windows hooks/filter, evdev, and gamepad all feed the same `SemanticInputProducer` and sequence domain. Replace `run_input_forwarding_loop` and `run_windows_driver_capture_loop` forwarding logic with capture producers. Forward driver raw relative deltas without first converting them through the source absolute cursor while remotely active. Configure the 1000 Hz Windows low-latency path without the portable 5 ms mouse debounce.

For each `PeerInbound`, derive `AuthenticatedInputOwner` from its `PeerAuthContext` and submit realtime/reliable receivers directly to `InputInjectionHandle`; never trust an owner from the wire. Closed `ControlFrame`/`TelemetryFrame` handling stays on the existing non-input event tasks. A disconnect is applied only when its `ControlConnectionId` still matches the registered generation.

Migrate endpoint injection and mobile gateway injection to the same `InputInjectionHandle`. Disconnect, connection error, backend degradation, lease expiry, shutdown, lock/sleep, and ownership transfer all call `request_release_all`.

**Step 4: Remove the legacy peer-input path**

After every producer/consumer uses typed lanes, delete peer-network variants and adapters for:

- `Message::MouseMove`, `MouseButton`, `MouseWheel`, `Key`, `KeyExtended`;
- `Message::GamepadState`, `ScreenEnter`, `ScreenLeave`;
- the old realtime `Message` codec and reliable mouse fallback;
- production `InputEventChannel`, `send_forwarded_messages`, and `inject_remote_message`;
- production `Arc<Mutex<Box<dyn InjectBackend>>>`.

Keep local/mobile IPC domain events only where they are not peer-wire messages. Add `legacy_peer_input_is_rejected` and this static check:

```powershell
rg -n "Message::(MouseMove|MouseButton|MouseWheel|Key|KeyExtended|GamepadState|ScreenEnter|ScreenLeave)" apps crates
```

Expected after migration: no peer send/receive production match.

Rerun `inbound_v2_hello_is_rejected_before_registration` and `old_peer_can_complete_tls_then_receive_explicit_version_rejection` here, after the last legacy peer-input parser has been deleted, to prove the compatibility bootstrap still emits `HelloRejected` before any `RSQ3` lane is required.

**Step 5: Run daemon and runtime contracts**

Run:

```powershell
cargo test -p rshare-daemon input_runtime
cargo test -p rshare-daemon --test input_pipeline_integration
cargo test -p rshare-daemon
cargo test -p rshare-core --test runtime_contract
cargo test -p rshare-core --test session_state_machine
cargo test -p rshare-net --test protocol_v3_handshake old_peer_can_complete_tls_then_receive_explicit_version_rejection
```

Expected: all tests PASS and old tests for layout routing now exercise the actor fixture.

**Step 6: Commit**

```powershell
git add apps/rshare-daemon/Cargo.toml apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/input_runtime.rs apps/rshare-daemon/src/input_state.rs apps/rshare-daemon/src/endpoint_runtime.rs apps/rshare-daemon/src/mobile_gateway.rs apps/rshare-daemon/src/main.rs apps/rshare-daemon/tests/input_pipeline_integration.rs crates/rshare-core/src/protocol/mod.rs crates/rshare-input/src/listener.rs crates/rshare-input/src/gamepad.rs
git commit -m "refactor: unify daemon input routing actor"
```

### Task 13: Remove Per-Event Diagnostics from the Control Path

**Files:**
- Create: `apps/rshare-daemon/src/diagnostics_runtime.rs`
- Modify: `apps/rshare-daemon/src/lib.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Create: `apps/rshare-daemon/tests/diagnostics_isolation.rs`

**Step 1: Write failing sampled-diagnostics tests**

Directly instantiate the not-yet-existing `DiagnosticsRuntime` through the daemon library and prove its new behavior:

- with no subscribers, 100,000 input metric updates perform zero formatting/sends;
- with one subscriber and fake time, sampling occurs at 20 Hz rather than once per input;
- the recent-event ring stays at its declared capacity and continuous motion is sampled/coalesced;
- subscribe/unsubscribe gates delivery immediately;
- one physical source input can contribute to counters but never generates two peer telemetry messages.

Keep the blocked-sink integration assertion as a regression check: block the diagnostics sink for five seconds while routing 100,000 mouse events plus a key barrier, then assert bounded input depth, latest pointer delivery, and reliable key delivery. That isolation should already pass after Task 12; the RED condition here is the missing sampled/subscriber-aware runtime and its bounded-history contract.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-daemon --test diagnostics_isolation
```

Expected: FAIL because `DiagnosticsRuntime`, 20 Hz sampling, subscriber gating, and the bounded recent-event ring do not exist. A failure solely due to input-path blocking would mean Task 12 regressed and must be fixed there.

**Step 3: Add sampled subscriber-aware diagnostics**

`InputRuntime` records only fixed-size counters/state through Task 12's `ControlMetrics` atomics/watch. `DiagnosticsRuntime` samples `ControlMetrics::snapshot()` at 20 Hz, publishes the resulting `ControlMetricSnapshot` on a watch channel, formats strings/JSON there, and sends only to explicit subscribers:

```rust
pub struct DiagnosticsRuntime {
    latest: watch::Receiver<ControlMetricSnapshot>,
    subscribers: Arc<SubscriptionRegistry>,
    interval: tokio::time::Interval,
}
```

Delete the per-event `broadcast_diagnostic_event(...).await` path. Preserve a bounded `VecDeque` recent-event ring; continuous motion is sampled rather than appended at 1000 Hz.

**Step 4: Run tests and commit**

Run:

```powershell
cargo test -p rshare-daemon --test diagnostics_isolation
cargo test -p rshare-daemon
```

Expected: all tests PASS.

```powershell
git add apps/rshare-daemon/src/diagnostics_runtime.rs apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/main.rs apps/rshare-daemon/tests/diagnostics_isolation.rs
git commit -m "perf: isolate sampled input diagnostics"
```

### Task 13A: Give the Windows Kernel Queue Reliable Input Semantics

**Files:**
- Create: `drivers/windows/rshare-filter/event_queue.h`
- Create: `drivers/windows/rshare-filter/event_queue.c`
- Create: `drivers/windows/rshare-filter/tests/event_queue_tests.c`
- Modify: `drivers/windows/rshare-filter/rshare-filter.vcxproj`
- Modify: `drivers/windows/rshare-filter/driver.c`
- Modify: `drivers/windows/rshare-common/rshare_ioctls.h`
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Modify: `apps/rshare-daemon/src/input_runtime.rs`
- Modify: `crates/rshare-platform/tests/windows_driver_smoke.rs`
- Modify: `apps/rshare-daemon/tests/input_pipeline_integration.rs`
- Create: `scripts/driver/test-filter-queue.ps1`
- Modify: `scripts/driver/validate-hid.ps1`

**Step 1: Write failing portable C queue tests**

Require:

- adjacent relative motion accumulates;
- coalescing never crosses key/button/wheel barriers;
- discrete events are never silently evicted;
- a full all-discrete queue latches `RSHARE_EVENT_RELIABLE_OVERFLOW` outside the ordinary ring;
- stats distinguish coalesced realtime, dropped realtime, and reliable overflow.

**Step 2: Run and verify failure**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/driver/test-filter-queue.ps1
```

Expected: FAIL because the semantic queue unit and test script do not exist.

**Step 3: Extract and implement the semantic queue**

Use:

```c
typedef enum _RSHARE_EVENT_QUEUE_PUSH_RESULT {
    RShareEventQueued,
    RShareRealtimeCoalesced,
    RShareRealtimeDropped,
    RShareReliableOverflowLatched
} RSHARE_EVENT_QUEUE_PUSH_RESULT;

RSHARE_EVENT_QUEUE_PUSH_RESULT
RShareEventQueuePush(
    RSHARE_EVENT_QUEUE* queue,
    const RSHARE_DRIVER_EVENT* event);

BOOLEAN
RShareEventQueuePop(
    RSHARE_EVENT_QUEUE* queue,
    RSHARE_DRIVER_EVENT* event);
```

The driver calls this unit under its spinlock. Increase only the filter driver version to 0.4.0; keep the shared `RSHARE_DRIVER_ABI` at 1 so vhid/vdisplay packages and existing `virtual_display.rs` clients remain compatible. Add a filter-specific `RSHARE_FILTER_EVENT_FORMAT_VERSION = 2`, `RSHARE_CAP_FILTER_SEMANTIC_QUEUE`, and an independently sized/versioned `RSHARE_FILTER_STATS_V2` plus query IOCTL instead of changing the common stats struct in place. Extend filter package/static assertions and add `RSHARE_EVENT_RELIABLE_OVERFLOW`; use `KeQueryPerformanceCounter` for monotonic capture timestamps.

`rshare-platform` exposes the overflow as a platform-neutral raw capture status and does not depend on `rshare-input`. The `rshare-input` Windows capture adapter maps that status to `IngressFault::ReliableOverflow`; Task 12's daemon runtime then suspends, advances epoch, and invokes the targeted emergency `ReleaseAll`. Add an integration test for this entire mapping.

**Step 4: Run queue/ABI tests and commit**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/driver/test-filter-queue.ps1
cargo test -p rshare-platform windows_driver_abi
powershell -ExecutionPolicy Bypass -File scripts/driver/build.ps1 -Configuration Debug -Platform x64
```

Expected: all tests/build PASS.

```powershell
git add drivers/windows/rshare-filter/event_queue.h drivers/windows/rshare-filter/event_queue.c drivers/windows/rshare-filter/tests/event_queue_tests.c drivers/windows/rshare-filter/rshare-filter.vcxproj drivers/windows/rshare-filter/driver.c drivers/windows/rshare-common/rshare_ioctls.h crates/rshare-platform/src/windows.rs crates/rshare-input/src/backend.rs apps/rshare-daemon/src/input_runtime.rs crates/rshare-platform/tests/windows_driver_smoke.rs apps/rshare-daemon/tests/input_pipeline_integration.rs scripts/driver/test-filter-queue.ps1 scripts/driver/validate-hid.ps1
git commit -m "fix: preserve reliable filter queue events"
```

### Task 13B: Replace Both Windows Poll Loops with a Cancellable Wait

**Files:**
- Modify: `drivers/windows/rshare-common/rshare_ioctls.h`
- Modify: `drivers/windows/rshare-filter/driver.c`
- Modify: `drivers/windows/tools/rshare-driver-probe.c`
- Modify: `crates/rshare-platform/src/windows.rs`
- Modify: `crates/rshare-input/src/backend.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-platform/tests/windows_driver_smoke.rs`
- Modify: `drivers/windows/README.md`

**Step 1: Write failing ABI and live-smoke tests**

Add `RSHARE_CAP_WAIT_EVENT`, `IOCTL_RSHARE_WAIT_EVENT`, and:

```rust
#[test]
#[ignore = "requires installed rshare-filter driver"]
fn wait_event_wakes_without_polling_delay() {
    let (stream, cancel) = WindowsDriverEventStream::open_filter().unwrap();
    let waiter = std::thread::spawn(move || {
        let mut stream = stream;
        stream.wait_event()
    });
    assert!(wait_until_filter_reports_pending_waiter(Duration::from_secs(1)));
    trigger_driver_test_mouse_event();
    let event = waiter.join().unwrap().unwrap();
    assert_eq!(event.event_kind, WindowsDriverEventKind::MouseMove);
    drop(cancel);
}

#[test]
#[ignore = "requires installed rshare-filter driver"]
fn pending_wait_can_be_cancelled_from_another_thread() {
    let (stream, cancel) = WindowsDriverEventStream::open_filter().unwrap();
    let waiter = std::thread::spawn(move || {
        let mut stream = stream;
        stream.wait_event()
    });
    assert!(wait_until_filter_reports_pending_waiter(Duration::from_secs(1)));
    cancel.cancel().unwrap();
    assert!(matches!(waiter.join().unwrap(), Err(DriverWaitError::Cancelled)));
}
```

Keep a separate immediate-return test that queues an event before waiting. The two tests above must observe the filter-specific pending-waiter counter before emit/cancel, so they cover the empty/pend race rather than accidentally exercising only the immediate path. Static tests must find no empty-queue 16 ms daemon sleep, 1 ms `VirtualHidCaptureDriver` sleep, or probe `Sleep(10)`.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-platform windows_filter_capture_uses_wait_event
cargo test -p rshare-input virtual_hid_capture_uses_wait_event
```

Expected: FAIL because both production readers poll.

**Step 3: Add the KMDF pending-request boundary**

The control context gets a separate `WdfIoQueueDispatchManual` pending-read queue. Do not pend on the existing sequential default queue, which would block version/stats IOCTLs.

Required behavior:

1. `WAIT_EVENT` completes immediately when an event exists.
2. Otherwise it forwards one cancellable request to the manual queue.
3. After forwarding, drain again to close the check-empty/pend race.
4. A producer satisfies a pending waiter after enqueue.
5. file cleanup/cancel completes the corresponding request with `STATUS_CANCELLED`.
6. `READ_EVENT` remains only for compatibility/diagnostics.

Expose:

```rust
pub struct WindowsDriverEventStream;
#[derive(Clone)]
pub struct WindowsDriverEventStreamCancel;

impl WindowsDriverEventStream {
    pub fn open_filter() -> Result<(Self, WindowsDriverEventStreamCancel)>;
    pub fn wait_event(&mut self) -> Result<WindowsDriverInputEvent>;
}

impl WindowsDriverEventStreamCancel {
    pub fn cancel(&self) -> Result<()>;
}
```

Open with `FILE_FLAG_OVERLAPPED`, use overlapped `DeviceIoControl`, and keep the current wait's heap-stable `OVERLAPPED` registration in shared state. The separately owned cloneable cancel handle calls `CancelIoEx` for that exact registration from another thread. Clear registration after completion and cover cancel-before-register, cancel-during-register, completion-vs-cancel, and repeated shutdown races. Shutdown must not depend on a polling timeout.

Update both `run_windows_driver_capture_loop` and `VirtualHidCaptureDriver::event_loop`. Update the probe watch command to wait rather than sleep.

**Step 4: Build, test, and run the Phase 1 control gate**

Run:

```powershell
cargo test -p rshare-platform
cargo test -p rshare-input
cargo test -p rshare-daemon
powershell -ExecutionPolicy Bypass -File scripts/driver/build.ps1 -Configuration Debug -Platform x64
cargo test -p rshare-platform --test windows_driver_smoke wait_event_ -- --ignored --nocapture --test-threads=1
cargo run --release -p rshare-perf -- quic --rate-hz 1000 --duration-secs 60 --load diagnostics,status,audio,bulk --output C:\tmp\rshare-perf\phase1-control.json
```

Expected:

- all correctness tests PASS;
- zero lost reliable frames and zero stuck state;
- every queue stays within its declared bound;
- stale realtime replay is zero;
- concurrent load adds no more than 5 ms to loopback input p99;
- slow-peer load adds no more than 2 ms to fast-peer p99.

Loopback is a regression/isolation gate, not proof of the wired two-machine 3/6/10 ms SLO. That absolute gate remains Task 30. If this gate fails, preserve JSON artifacts and stop before UI/media.

**Step 5: Commit**

```powershell
git add drivers/windows/rshare-common/rshare_ioctls.h drivers/windows/rshare-filter/driver.c drivers/windows/tools/rshare-driver-probe.c crates/rshare-platform/src/windows.rs crates/rshare-platform/tests/windows_driver_smoke.rs crates/rshare-input/src/backend.rs apps/rshare-daemon/src/main.rs drivers/windows/README.md
git commit -m "perf: wake windows filter capture events"
```

### Task 14: Replace Newline IPC with Bounded Framed I/O

**Files:**
- Modify: `crates/rshare-core/Cargo.toml`
- Create: `crates/rshare-core/src/ipc_frame.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/tests/ipc_contract.rs`
- Create: `crates/rshare-core/tests/ipc_framing_contract.rs`
- Modify: `apps/rshare-daemon/src/lib.rs`
- Create: `apps/rshare-daemon/src/ipc_server.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-desktop-frontend/vite.config.ts`
- Create: `apps/rshare-desktop-frontend/src/app/ipc-frame.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/ipc-frame.test.mjs`
- Modify: `tools/rshare-perf/Cargo.toml`
- Modify: `tools/rshare-perf/src/main.rs`
- Create: `tools/rshare-perf/src/ipc.rs`

**Step 1: Write failing frame tests**

Cover fragmented reads, multiple frames on one connection, oversized rejection, unexpected kind rejection, and a multi-megabyte binary payload:

```rust
#[tokio::test]
async fn fragmented_header_and_body_decode_one_frame() {
    let payload = vec![7_u8; 2 * 1024 * 1024];
    let mut source = CountingChunkReader::new(encode_frame(IpcEnvelopeKind::Binary, &payload), 4096);
    let frame = IpcFrameCodec::default().read_frame(&mut source).await.unwrap().unwrap();
    assert_eq!(frame.payload, payload);
    assert!(source.read_calls() < 600);
}

#[tokio::test]
async fn oversized_binary_frame_is_rejected_before_body_read() {
    // Header declares MAX_IPC_FRAME_BYTES + 1; body is absent.
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core --test ipc_framing_contract
```

Expected: FAIL because only newline JSON helpers exist.

**Step 3: Implement the frame codec**

Use:

```rust
pub const IPC_FRAME_HEADER_LEN: usize = 5;
pub const DEFAULT_MAX_JSON_FRAME_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_BINARY_FRAME_BYTES: usize = 32 * 1024 * 1024;

#[repr(u8)]
pub enum IpcEnvelopeKind {
    Json = 1,
    Binary = 2,
    UiState = 3,
    Heartbeat = 4,
}

pub struct IpcFrame {
    pub kind: IpcEnvelopeKind,
    pub payload: bytes::Bytes,
}

pub struct IpcFrameCodec {
    pub limits: IpcFrameLimits,
}
```

Wire format:

```text
u32 payload_length (big endian)
u8  envelope_kind
N   payload
```

`payload_length` excludes the kind byte. Use `read_exact` for the five-byte header and one bounded allocation/read for payload. Reject length before allocation using a per-kind limit. EOF before any header byte is `Ok(None)`; partial header/body is `UnexpectedEof`. Write with `write_all`; flush once per logical response.

Add JavaScript `encodeIpcFrame`/`IpcFrameDecoder` with the same partial/back-to-back/oversized behavior. The Vite bridge stops appending/searching for newlines.

Export `ipc_server` from `apps/rshare-daemon/src/lib.rs` and add the daemon library dependency to `tools/rshare-perf/Cargo.toml`. Extract a library-visible connection handler so tests bind `127.0.0.1:0`; do not contend on fixed port 27435 or reach into private `main.rs`. Update all daemon, CLI, Tauri, Vite, and existing `crates/rshare-core/tests/ipc_contract.rs` request paths to framed JSON. Delete production and test uses of `read_json_line`/`write_json_line` after the migration; do not implement dual-protocol guessing.

Add `rshare-perf ipc` scenarios that spawn the real handler on an ephemeral port:

- 500 sequential requests;
- 8×500 concurrent requests;
- persistent framed requests where supported.

**Step 4: Run IPC, daemon, CLI, and desktop Rust tests**

Run:

```powershell
cargo test -p rshare-core --test ipc_framing_contract
cargo test -p rshare-daemon
cargo test -p rshare-cli
cargo test -p rshare-desktop
npm.cmd --prefix apps/rshare-desktop-frontend test
cargo run --release -p rshare-perf -- ipc --requests 500 --concurrency 1,8 --output C:\tmp\rshare-perf\ipc.json
```

Expected: all tests PASS; IPC load test shows no one-byte read loop.

**Step 5: Commit**

```powershell
git add crates/rshare-core/Cargo.toml crates/rshare-core/src/ipc_frame.rs crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs crates/rshare-core/src/lib.rs crates/rshare-core/tests/ipc_contract.rs crates/rshare-core/tests/ipc_framing_contract.rs apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/ipc_server.rs apps/rshare-daemon/src/main.rs apps/rshare-desktop-frontend/vite.config.ts apps/rshare-desktop-frontend/src/app/ipc-frame.mjs apps/rshare-desktop-frontend/src/app/ipc-frame.test.mjs tools/rshare-perf/Cargo.toml tools/rshare-perf/src/main.rs tools/rshare-perf/src/ipc.rs
git commit -m "refactor: use bounded framed daemon ipc"
```

### Task 15: Add Versioned UiSnapshot, UiDelta, and Resync Contracts

**Files:**
- Create: `crates/rshare-core/src/ui_state.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Test: `crates/rshare-core/src/ui_state.rs`
- Test: `crates/rshare-core/tests/ipc_contract.rs`

**Step 1: Write failing revision contract tests**

Require:

- snapshot has `boot_id` and revision;
- each applied delta increments by exactly one;
- heartbeat and overwritten-but-never-emitted latest values do not consume revisions;
- wrong `boot_id` or a revision gap returns `ResyncRequired`;
- pointer-only delta does not alter topology;
- serialized ordinary delta remains below 1 KiB in the fixture.

```rust
#[test]
fn revision_gap_requires_full_snapshot() {
    let mut view = UiView::from_snapshot(snapshot(BOOT, 7));
    assert_eq!(
        view.apply(UiEnvelope::Delta(delta(BOOT, 9, pointer_change()))),
        Err(UiApplyError::RevisionGap { expected: 8, actual: 9 })
    );
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core ui_state::tests
```

Expected: FAIL because UI state contracts do not exist.

**Step 3: Implement typed envelopes**

Define:

```rust
pub const UI_STATE_PROTOCOL_VERSION: u16 = 1;

pub struct UiCursor {
    pub boot_id: Uuid,
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiSnapshot {
    pub protocol_version: u16,
    pub boot_id: Uuid,
    pub revision: u64,
    pub status: ServiceStatusSnapshot,
    pub devices: Vec<DaemonDeviceSnapshot>,
    pub layout: LayoutGraph,
    pub capabilities: CapabilityRegistrySnapshot,
    pub display_inventory: LocalDisplayState,
    pub dynamic_state: UiDynamicState,
    pub active_sessions: UiActiveSessions,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiDelta {
    pub boot_id: Uuid,
    pub revision: u64,
    pub change: UiChange,
}

pub enum UiEnvelope {
    Snapshot(UiSnapshot),
    Delta(UiDelta),
    ResyncRequired {
        boot_id: Uuid,
        current_revision: u64,
        reason: UiResyncReason,
    },
    Heartbeat { boot_id: Uuid, revision: u64, sent_at_ms: u64 },
}
```

`UiChange` must be typed: status/capabilities, device upsert/remove, topology/display, pointer/gamepad latest state, key/button transition, session, diagnostics/latency, and media-session state. Do not use `serde_json::Value`.

An initial snapshot starts at revision 0. Only an envelope actually emitted as a delta increments revision. A heartbeat repeats the latest revision. A pointer/gamepad value overwritten in a `watch` slot before emission never creates a hidden revision gap. Media session lists use `#[serde(default)]` so older saved/local snapshots decode to an empty list.

**Step 4: Run contract tests**

Run:

```powershell
cargo test -p rshare-core ui_state
cargo test -p rshare-core --test ipc_contract
```

Expected: all tests PASS.

**Step 5: Commit**

```powershell
git add crates/rshare-core/src/ui_state.rs crates/rshare-core/src/lib.rs crates/rshare-core/src/ipc.rs crates/rshare-core/tests/ipc_contract.rs
git commit -m "feat: define revisioned ui state protocol"
```

### Task 16: Add the Daemon StateAggregator and One Persistent UI Stream

**Files:**
- Create: `apps/rshare-daemon/src/state_aggregator.rs`
- Modify: `apps/rshare-daemon/src/input_state.rs`
- Modify: `apps/rshare-daemon/src/lib.rs`
- Modify: `apps/rshare-daemon/src/ipc_server.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Test: `apps/rshare-daemon/src/state_aggregator.rs`
- Create: `apps/rshare-daemon/tests/ipc_load.rs`

**Step 1: Write failing aggregator/stream tests**

Prove:

- one consistent initial snapshot;
- contiguous revisions for state changes;
- pointer latest-value updates cannot fill the discrete event history;
- a lagged subscriber receives `ResyncRequired`;
- a saturated reliable mutation lane rebuilds from the authoritative projection and the replacement snapshot contains the mutation;
- daemon restart changes `boot_id`;
- no interval-based state refresh is needed while the stream is live.

```rust
#[tokio::test]
async fn lagged_subscriber_is_told_to_resync() {
    let aggregator = StateAggregator::new(fixture_snapshot(), 4);
    let mut subscriber = aggregator.subscribe(None).await;
    for index in 0..20 {
        aggregator
            .publish(StateChange::DeviceUpsert(device_with_revision(index)))
            .await
            .unwrap();
    }
    assert!(matches!(
        subscriber.recv().await.unwrap(),
        UiEnvelope::ResyncRequired { .. }
    ));
}

#[tokio::test]
async fn pointer_flood_uses_latest_slot_without_filling_reliable_history() {
    let (input, feeds) = input_state_channel(4);
    let aggregator = StateAggregator::with_input(fixture_snapshot(), 4, feeds);
    for x in 0..100_000 {
        input.publish_pointer(input_pointer(x));
    }
    assert_eq!(aggregator.reliable_history_len(), 0);
    assert_eq!(aggregator.latest_pointer().unwrap().x, 99_999);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-daemon state_aggregator::tests
```

Expected: FAIL because state projection currently lives in the global `DaemonState`.

**Step 3: Implement the single-owner aggregator**

Implement:

```rust
pub enum StateChange {
    Status(ServiceStatusSnapshot),
    DeviceUpsert(DaemonDeviceSnapshot),
    DeviceRemove(DeviceId),
    Layout(LayoutGraph),
    Capabilities(CapabilityRegistrySnapshot),
    DisplayInventory(LocalDisplayState),
    Pointer(UiPointerState),
    KeyButton(UiDiscreteInputState),
    Session(UiActiveSessions),
    Diagnostics(LatencyFeedbackSnapshot),
    Media(UiMediaSessionState),
}

#[derive(Clone)]
pub struct StateAggregatorHandle {
    latest: watch::Receiver<Arc<UiSnapshot>>,
    deltas: broadcast::Sender<UiDelta>,
}

pub struct StateAggregatorInputs {
    pub reliable_tx: mpsc::Sender<StateChange>,
    pub gamepads_tx: watch::Sender<Option<Vec<LocalGamepadState>>>,
    pub input: InputStateFeeds,
    pub projection: Arc<dyn UiProjectionSource>,
}
```

The aggregator task is the only revision writer. Static inventory changes only on OS notifications, explicit commands, or low-frequency TTL refresh. Dynamic pointer/gamepad state uses `watch`/latest-value semantics. Implement the full `UiProjectionSource` adapter over Task 12's `InputStatePublisher` plus the other daemon actor projections; do not introduce a second input-state type. Reliable mutations use a bounded channel and cannot be silently lost: non-hot producers await capacity. Every hot producer first publishes its current authoritative projection through an actor-owned `ArcSwap`/`watch` snapshot, then tries to enqueue the delta. If `try_send` fails, the Task-12 dirty notifier wakes the aggregator; the aggregator stops emitting deltas, obtains a consistent projection from all source actors, emits `ResyncRequired`, and publishes a rebuilt snapshot containing the mutation. It must never reuse its old snapshot as the rebuild source. Keep a default 1024-delta replay ring and bounded discrete-event history.

`DirtyProjectionNotifier` coalesces repeated dirty signals but cannot itself be lost. Tests fill the reliable channel, apply a key/session mutation only to the authoritative source, then verify the rebuilt snapshot contains it and no false contiguous delta is emitted.

Add `DaemonRequest::SubscribeUiState { cursor: Option<UiCursor> }`. A same-boot cursor inside replay history resumes from missing deltas; an expired/different-boot cursor receives `ResyncRequired` immediately followed by the latest snapshot. `daemon_client::subscribe_ui_state` opens one framed TCP connection and yields envelopes until disconnect.

Replace dashboard/local-control/endpoint event streams as authoritative state sources. Keep old requests temporarily as disconnected fallback until Task 18.

**Step 4: Run daemon/client tests**

Run:

```powershell
cargo test -p rshare-daemon state_aggregator
cargo test -p rshare-daemon --test ipc_load
cargo test -p rshare-core daemon_client
```

Expected: all tests PASS; a single subscriber connection receives snapshot plus deltas.

**Step 5: Commit**

```powershell
git add apps/rshare-daemon/src/state_aggregator.rs apps/rshare-daemon/src/input_state.rs apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/ipc_server.rs apps/rshare-daemon/src/main.rs crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs apps/rshare-daemon/tests/ipc_load.rs
git commit -m "feat: stream authoritative daemon ui state"
```

### Task 17: Unify Tauri and Browser UI State Clients

**Files:**
- Modify: `apps/rshare-daemon/Cargo.toml`
- Modify: `apps/rshare-daemon/src/lib.rs`
- Create: `apps/rshare-daemon/src/ui_state_server.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Create: `apps/rshare-daemon/tests/ui_state_websocket.rs`
- Create: `apps/rshare-desktop/src-tauri/src/ui_state_bridge.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Create: `apps/rshare-desktop-frontend/src/app/ui-state-client.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/ui-state-client.test.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Test: `apps/rshare-desktop/src-tauri/src/main.rs`

**Step 1: Write failing transport-equivalence tests**

In Node tests, run the same envelope sequence through fake Tauri and WebSocket transports and assert identical callbacks/state. Cover reconnect, heartbeat timeout, boot change, and revision gap.

```javascript
test("tauri and websocket transports expose identical envelope behavior", async () => {
  const envelopes = [snapshot(BOOT, 1), pointerDelta(BOOT, 2), heartbeat(BOOT, 2)];
  assert.deepEqual(
    await collectWithTransport(fakeTauriTransport(envelopes)),
    await collectWithTransport(fakeWebSocketTransport(envelopes)),
  );
});
```

Add a Rust bridge test proving `start_ui_state_stream` starts even when the Vite gateway is available; the network-command shortcut must not swallow stream startup.

**Step 2: Run and verify failure**

Run:

```powershell
node --test apps/rshare-desktop-frontend/src/app/ui-state-client.test.mjs
cargo test -p rshare-desktop ui_state_stream
```

Expected: FAIL because stream behavior is split between local-controls, endpoint, gateway, and Tauri event code.

**Step 3: Implement one transport-independent client**

Expose:

```javascript
export class UiStateClient {
  constructor({ connect, onEnvelope, onStatus, heartbeatTimeoutMs = 12000 }) {}
  async start() {}
  async stop() {}
  currentRevision() {}
}
```

The Tauri command `start_ui_state_stream` proxies `daemon_client::subscribe_ui_state` and emits one `"rshare://ui-state"` event. A second start cancels and awaits the prior task; stop is idempotent.

For browser development, add a loopback-only WebSocket server route in `ui_state_server.rs`. Upgrade with `accept_hdr_async`, validate the request path and origin, and route `/ui-state` to the same `StateAggregator` subscription while preserving `/local-controls` behavior. Reject unknown paths before exposing state. The Vite configuration connects/proxies to this real route; it does not fabricate state or treat stream start/stop as a no-op network command. `ui_state_websocket.rs` binds an ephemeral loopback port and proves snapshot/delta/resync behavior and path rejection.

Reconnect uses bounded delays 100, 250, 500, then 1000 ms maximum. Heartbeats are emitted every 5 seconds; 12 seconds without any envelope marks the last snapshot stale. The client retains that snapshot, requests exactly one full resync after a boot/revision gap, and never merges an invalid delta. A disconnected/stale stream waits one second before enabling one low-frequency single-flight fallback poll; the next valid snapshot stops fallback immediately.

**Step 4: Run frontend and Tauri tests**

Run:

```powershell
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
cargo test -p rshare-desktop
cargo test -p rshare-daemon --test ui_state_websocket
```

Expected: all tests/build PASS.

**Step 5: Commit**

```powershell
git add apps/rshare-daemon/Cargo.toml apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/ui_state_server.rs apps/rshare-daemon/src/main.rs apps/rshare-daemon/tests/ui_state_websocket.rs apps/rshare-desktop/src-tauri/src/ui_state_bridge.rs apps/rshare-desktop/src-tauri/src/main.rs apps/rshare-desktop-frontend/src/app/ui-state-client.mjs apps/rshare-desktop-frontend/src/app/ui-state-client.test.mjs apps/rshare-desktop-frontend/src/app/App.tsx
git commit -m "feat: unify desktop ui state streaming"
```

### Task 18: Add Slice Stores and RAF Latest-Value Rendering

**Files:**
- Create: `apps/rshare-desktop-frontend/src/app/ui-store.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/ui-store.test.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/use-ui-store.ts`
- Modify: `apps/rshare-desktop-frontend/package.json`
- Modify: `apps/rshare-desktop-frontend/package-lock.json`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/components/MonitorManager.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/desktop-model.mjs`
- Test: `apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs`

**Step 1: Write failing selector/render tests**

Prove:

- pointer-only deltas do not notify topology subscribers;
- 1000 pointer deltas before one RAF produce one pointer-store commit;
- key/button transitions are applied immediately and in order;
- equal `externalDevices` topology does not trigger `MonitorManager` state replacement;
- healthy push produces zero dashboard/endpoint poll calls over ten simulated seconds.

```javascript
test("pointer flood commits once and leaves topology untouched", () => {
  const raf = fakeRaf();
  const store = createUiStore(snapshot());
  let topologyCommits = 0;
  let pointerCommits = 0;
  store.subscribe(selectTopology, () => topologyCommits++);
  store.subscribe(selectPointer, () => pointerCommits++);
  for (let x = 0; x < 1000; x++) store.apply(pointerDelta(x), raf.schedule);
  raf.flush();
  assert.equal(pointerCommits, 1);
  assert.equal(topologyCommits, 0);
});
```

**Step 2: Run and verify failure**

Run:

```powershell
node --test apps/rshare-desktop-frontend/src/app/ui-store.test.mjs
```

Expected: FAIL because high-frequency events replace root `localControls`.

**Step 3: Implement selector-driven slices**

`createUiStore` owns:

- topology/layout;
- connections/capabilities;
- local/remote input visuals;
- diagnostics/latency;
- media session.

Use `useSyncExternalStore` in `use-ui-store.ts`. Continuous pointer/gamepad deltas replace a pending slot and commit once in `requestAnimationFrame`; discrete transitions commit immediately. Memoize topology projection by topology revision.

Add `react-test-renderer = "18.3.1"` as a dev dependency and assert a selector component does not render for an unrelated slice. Applying a full snapshot cancels pending stale RAF work before atomically replacing slices.

Remove `MonitorManager`'s unconditional `setDevices` mirror. It must render directly from a stable selector value or compare topology revision before updating local edit state.

**Step 4: Disable healthy polling**

When `UiStateClient` status is `healthy`, do not schedule:

- 1500 ms dashboard polling;
- 750 ms endpoint polling;
- duplicate local-controls/endpoint streams.

Keep only a 5-second heartbeat watchdog. When disconnected, use a single-flight 2-second fallback snapshot request until the stream reconnects. Tray polling is out of renderer scope and remains low-frequency unless its state is also subscribed.

**Step 5: Run tests and build**

Run:

```powershell
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
```

Expected: all  existing and new tests PASS; the healthy-stream timer test records zero dashboard/endpoint calls.

**Step 6: Commit**

```powershell
git add apps/rshare-desktop-frontend/package.json apps/rshare-desktop-frontend/package-lock.json apps/rshare-desktop-frontend/src/app/ui-store.mjs apps/rshare-desktop-frontend/src/app/ui-store.test.mjs apps/rshare-desktop-frontend/src/app/use-ui-store.ts apps/rshare-desktop-frontend/src/app/App.tsx apps/rshare-desktop-frontend/src/app/components/MonitorManager.tsx apps/rshare-desktop-frontend/src/app/desktop-model.mjs apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs
git commit -m "perf: isolate high frequency desktop state"
```

### Task 19: Send Static Display Previews as Compressed Binary

**Files:**
- Create: `crates/rshare-core/src/ipc_binary.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/src/local_controls.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Modify: `crates/rshare-platform/Cargo.toml`
- Create: `crates/rshare-platform/src/display_capture.rs`
- Modify: `crates/rshare-platform/src/lib.rs`
- Modify: `crates/rshare-platform/src/display.rs`
- Modify: `crates/rshare-platform/src/windows.rs`
- Create: `apps/rshare-daemon/src/static_capture.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Modify: `apps/rshare-desktop-frontend/vite.config.ts`
- Create: `apps/rshare-desktop-frontend/src/app/display-capture.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/display-capture.test.mjs`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Test: `crates/rshare-core/tests/ipc_contract.rs`
- Test: `apps/rshare-desktop/src-tauri/src/main.rs`
- Test: `apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs`

**Step 1: Write failing binary-preview tests**

Require:

- metadata JSON contains MIME type, dimensions, capture id, and byte length but no `Vec<u8>`;
- the next `Binary` frame has exactly that capture id/length;
- 900px preview is PNG/JPEG and fixture size is below 250 KiB;
- frontend creates/revokes a `Blob` URL and never builds a binary string/base64 data URL;
- capture worker concurrency is bounded.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core --test ipc_contract display_capture
cargo test -p rshare-desktop display_capture
node --test apps/rshare-desktop-frontend/src/app/display-capture.test.mjs apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs
```

Expected: FAIL because BMP bytes are embedded as a JSON number array and converted to base64.

**Step 3: Add compressed capture metadata plus binary body**

Replace the byte-bearing result with an explicit success/error wrapper:

```rust
pub struct DisplayCaptureResult {
    pub request_id: Uuid,
    pub status: DisplayOperationStatus,
    pub message: Option<String>,
    pub payload: Option<DisplayCaptureDescriptor>,
}

pub struct DisplayCaptureDescriptor {
    pub capture_id: Uuid,
    pub display_id: String,
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub byte_length: u32,
}

pub struct DisplayCaptureBlob {
    pub descriptor: DisplayCaptureDescriptor,
    pub bytes: bytes::Bytes,
}
```

On success, `status` is success and `payload` is `Some`; on failure, `payload` is `None`, `message` is nonempty, and no binary frame follows. The correlated binary frame begins with the 16 raw UUID bytes for `capture_id`, followed by compressed image bytes. The client rejects mismatched id or length.

Split platform capture into a BGRA/raw capture seam and encoder. Capture on `spawn_blocking`, cap concurrent captures with a semaphore of 2, and encode PNG by default (or JPEG quality 85 when explicitly selected). Add only `image` PNG/JPEG features. Linux paths that already return compressed PNG wrap it directly rather than decode/re-encode. Static previews remain user-triggered and have no refresh timer.

Daemon IPC writes a JSON response frame with metadata then a correlated binary frame. Tauri `capture_display_binary` and the Vite bridge both return the concatenated framed body as `application/octet-stream`/`tauri::ipc::Response`, so the renderer decodes an `ArrayBuffer`, creates `URL.createObjectURL(new Blob(...))`, and revokes old URLs on replacement/unmount. Multi-display capture uses concurrency limit 2.

**Step 4: Run all affected tests**

Run:

```powershell
cargo test -p rshare-core --test ipc_contract
cargo test -p rshare-platform display
cargo test -p rshare-daemon
cargo test -p rshare-desktop
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
```

Expected: all tests PASS and repository search finds no display-capture byte-array/base64 path:

```powershell
rg -n "String\\.fromCharCode|btoa\\(|data:image/bmp|Vec<u8>.*DisplayCapture" apps crates
```

Expected: no production matches.

**Step 5: Commit**

```powershell
git add crates/rshare-core/src/ipc_binary.rs crates/rshare-core/src/lib.rs crates/rshare-core/src/local_controls.rs crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs crates/rshare-core/tests/ipc_contract.rs crates/rshare-platform/Cargo.toml crates/rshare-platform/src/display_capture.rs crates/rshare-platform/src/lib.rs crates/rshare-platform/src/display.rs crates/rshare-platform/src/windows.rs apps/rshare-daemon/src/static_capture.rs apps/rshare-daemon/src/main.rs apps/rshare-desktop/src-tauri/src/main.rs apps/rshare-desktop-frontend/vite.config.ts apps/rshare-desktop-frontend/src/app/display-capture.mjs apps/rshare-desktop-frontend/src/app/display-capture.test.mjs apps/rshare-desktop-frontend/src/app/App.tsx apps/rshare-desktop-frontend/src/app/desktop-shell.test.mjs
git commit -m "perf: stream compressed display previews as binary"
```

### Task 20: Add the Phase 1 UI and Control Acceptance Gate

**Files:**
- Modify: `apps/rshare-desktop-frontend/package.json`
- Modify: `apps/rshare-desktop-frontend/package-lock.json`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/use-ui-store.ts`
- Modify: `apps/rshare-desktop/src-tauri/gen/schemas/acl-manifests.json`
- Modify: `apps/rshare-desktop/src-tauri/gen/schemas/desktop-schema.json`
- Modify: `apps/rshare-desktop/src-tauri/gen/schemas/windows-schema.json`
- Modify: `apps/rshare-daemon/src/input_runtime.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-daemon/tests/input_pipeline_integration.rs`
- Modify: `crates/rshare-net/src/connection.rs`
- Modify: `crates/rshare-net/src/network_manager.rs`
- Modify: `crates/rshare-net/src/qos.rs`
- Modify: `crates/rshare-net/src/transport.rs`
- Modify: `perf/baselines/schema.json`
- Modify: `tools/rshare-perf/src/compare.rs`
- Modify: `tools/rshare-perf/src/ipc.rs`
- Modify: `tools/rshare-perf/src/main.rs`
- Modify: `tools/rshare-perf/src/quic.rs`
- Modify: `tools/rshare-perf/src/report.rs`
- Create: `apps/rshare-desktop-frontend/playwright.perf.config.mjs`
- Create: `apps/rshare-desktop-frontend/tests/performance/ui-state.spec.mjs`
- Create: `scripts/perf/run-phase1.ps1`
- Create: `scripts/perf/collect-runner-fingerprint.ps1`
- Create: `docs/performance/phase1-windows-validation.md`
- Create: `.gitattributes`
- Create: `.github/workflows/performance-smoke.yml`
- Create: `.github/workflows/performance-nightly-windows.yml`

**Step 1: Write a failing UI performance scenario**

The Playwright scenario must feed 1000 Hz pointer deltas for ten seconds plus
100 discrete transitions and record real React committed-paint observations
from a passive effect:

- event-to-paint p50/p95/p99/max;
- React/topology commit counts;
- long tasks over 50 ms;
- outgoing dashboard/endpoint requests.

Assertions on the fixed Windows runner:

```javascript
expect(report.paint_p95_ms).toBeLessThanOrEqual(16.7);
expect(report.paint_p99_ms).toBeLessThanOrEqual(33);
expect(report.topology_commits_during_pointer_flood).toBe(0);
expect(report.long_tasks_over_50ms).toBe(0);
expect(report.dashboard_or_endpoint_polls_while_healthy).toBe(0);
```

Tag this test `@fixed-runner`; hosted CI runs functional stream tests but not strict paint timings.

**Step 2: Run and verify failure**

Run:

```powershell
npm.cmd --prefix apps/rshare-desktop-frontend run test:perf -- --grep @fixed-runner
```

Expected: FAIL because the script/config/scenario is not yet present.

**Step 3: Add the runner and CI split**

Add `@playwright/test = "1.55.0"` as a dev dependency and a `test:perf` script using `playwright.perf.config.mjs`.

`scripts/perf/run-phase1.ps1` must:

1. set a caller-selected output directory;
2. run all Rust correctness suites with `--locked`;
3. run QUIC 1000 Hz, slow/fast peer, and IPC harnesses as one exactly-five-run batch;
4. run the UI Playwright scenario as one exactly-five-run batch;
5. write every raw JSON plus the per-run latency samples and a combined summary with one batch id and the reproducibility fields from Task 2;
6. require a reviewed matching-runner baseline resolved and hash-verified through `perf/baselines/manifest.toml`, then compare through `rshare-perf compare --baseline-id ...`;
7. return nonzero on correctness loss, queue-bound violation, or threshold failure.

Hosted CI adds:

- `cargo test --workspace --locked`;
- `npm ci`, then frontend Node tests/build;
- `cargo bench --workspace --no-run --locked` compile-only;
- wide catastrophe-only smoke: 100k CPU path ≤5 s, IPC p99 ≤100 ms sequential/≤200 ms at concurrency 8, QUIC 125 Hz ×5 s reliable 100%/datagram ≥99.9%/p99 ≤100 ms;
- no cross-commit percentage comparison and no strict UI/GPU timing.

`performance-nightly-windows.yml` runs only on `[self-hosted, Windows, X64, rshare-perf]`, prevents overlapping jobs, uses `cargo --locked`/`npm ci`, records power plan/CPU affinity/runner fingerprint, warms 30 seconds, and runs exactly five complete same-config runs. It requires a matching baseline manifest entry from the protected default branch, verifies artifact/config hashes and the merged approval PR as specified in Task 2, and uploads all raw histograms. With CV >10%, it archives the whole unstable batch and reruns the entire five-run batch once; it never drops or selectively replaces a run. A second unstable batch fails. It restores the original power plan in `finally`; workflow code never auto-updates a baseline to match a regression.

Bootstrap mode may generate a candidate exactly-five-run artifact but must
return `PENDING_BASELINE`, never PASS. It writes a repository-shaped
`baseline-package` containing the canonical report, at least one nonempty raw
sidecar envelope, and an exact `manifest-entry.toml.template` whose hashes
refer to the packaged LF-normalized files. Promotion replaces only the
template's PR-reference placeholder. Put that package through the dedicated
reviewed/merged baseline checkpoint described under Delivery Map, then rerun
this task in strict mode from the updated protected branch. The strict
rerun—not the candidate-producing run—is the Phase 1 gate.

**Step 4: Commit the implementation**

Both Bootstrap and Strict reject a dirty worktree. Commit Task 20 before
measurement so every artifact is bound to one immutable implementation commit:

```powershell
git add .gitattributes apps/rshare-daemon/src/input_runtime.rs apps/rshare-daemon/src/main.rs apps/rshare-daemon/tests/input_pipeline_integration.rs apps/rshare-desktop-frontend/package.json apps/rshare-desktop-frontend/package-lock.json apps/rshare-desktop-frontend/src/app/App.tsx apps/rshare-desktop-frontend/src/app/use-ui-store.ts apps/rshare-desktop-frontend/playwright.perf.config.mjs apps/rshare-desktop-frontend/tests/performance apps/rshare-desktop/src-tauri/gen/schemas/acl-manifests.json apps/rshare-desktop/src-tauri/gen/schemas/desktop-schema.json apps/rshare-desktop/src-tauri/gen/schemas/windows-schema.json crates/rshare-net/src/connection.rs crates/rshare-net/src/network_manager.rs crates/rshare-net/src/qos.rs crates/rshare-net/src/transport.rs perf/baselines/schema.json tools/rshare-perf/src/compare.rs tools/rshare-perf/src/ipc.rs tools/rshare-perf/src/main.rs tools/rshare-perf/src/quic.rs tools/rshare-perf/src/report.rs scripts/perf/run-phase1.ps1 scripts/perf/collect-runner-fingerprint.ps1 docs/performance/phase1-windows-validation.md docs/plans/2026-07-28-low-latency-control-and-display-implementation-plan.md .github/workflows/performance-smoke.yml .github/workflows/performance-nightly-windows.yml
git commit -m "test: gate low latency control and ui state"
```

**Step 5: Run the complete Phase 1 gate**

Run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/perf/run-phase1.ps1 -OutputDirectory C:\tmp\rshare-phase1-results
```

Expected for the single-machine Phase 1 gate:

- zero reliable loss/duplication and zero stuck modifiers;
- 100 ms stall converges to latest state within 20 ms with zero stale replay;
- diagnostics/audio/status/bulk adds ≤5 ms to loopback input p99;
- slow peer adds ≤2 ms to fast-peer p99;
- matching fixed-runner median regression ≤10% and p95/p99 regression ≤15%;
- UI paint p95 ≤16.7 ms, p99 ≤33 ms;
- topology/status UI p95 ≤50 ms, p99 ≤100 ms;
- no healthy-stream dashboard/endpoint polling;
- all queues stay within declared bounds.

Label results `loopback`. Record 3/6/10 ms mouse and 8/15 ms reliable targets as informational only here; the wired two-machine absolute SLO is finalized exclusively in Task 30. An unstable or unavailable metric cannot pass.

Bootstrap must package all six scenario candidates into one dedicated draft
baseline PR. After its real PR number is substituted into all six templates,
merge the approved baseline PR, update/rebase onto the protected branch, and
run Strict from the resulting clean worktree. Bootstrap remains
`PENDING_BASELINE`; only that Strict rerun may pass Task 20.

### Task 21: Scaffold `rshare-media` Contracts and Bounded Newest-Frame Queues

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/rshare-media/Cargo.toml`
- Create: `crates/rshare-media/src/lib.rs`
- Create: `crates/rshare-media/src/types.rs`
- Create: `crates/rshare-media/src/queue.rs`
- Test: `crates/rshare-media/src/queue.rs`
- Test: `crates/rshare-media/src/types.rs`

**Step 1: Write failing media contract and queue tests**

Require:

- configuration rejects zero dimensions/fps, unsupported codecs, and excessive bitrate;
- one session selects exactly one physical or IDD display;
- a capacity-two raw/encoded queue drops the oldest replaceable frame and returns the newest;
- keyframe/control markers are never silently overwritten;
- bytes and frame count remain bounded under 100,000 pushes.

```rust
#[test]
fn newest_frame_queue_never_grows_past_capacity() {
    let queue = NewestFrameQueue::new(2, 8 * 1024 * 1024);
    for frame_id in 0..100_000 {
        queue.push(video_frame(frame_id, 16_000)).unwrap();
    }
    assert_eq!(queue.len(), 2);
    assert!(queue.bytes() <= 8 * 1024 * 1024);
    assert_eq!(queue.pop_newest().unwrap().frame_id, 99_999);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media
```

Expected: FAIL because the crate does not exist.

**Step 3: Add the crate and transport-neutral interfaces**

Define:

```rust
pub struct MediaSessionId(pub Uuid);

pub enum DisplaySourceKind {
    Physical,
    IndirectDisplay,
}

pub struct MediaConfig {
    pub display_id: String,
    pub source_kind: DisplaySourceKind,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    pub max_bitrate_bps: u32,
}

pub struct CapturedFrame {
    pub frame_id: u64,
    pub captured_at_us: u64,
    pub width: u32,
    pub height: u32,
    pub surface: VideoSurface,
}

pub struct EncodedFrame {
    pub frame_id: u64,
    pub pts_us: u64,
    pub keyframe: bool,
    pub bytes: bytes::Bytes,
}

pub trait FrameSource: Send {
    fn start(&mut self, config: &MediaConfig) -> MediaResult<()>;
    fn next_frame(&mut self, deadline: Instant) -> MediaResult<Option<CapturedFrame>>;
    fn stop(&mut self);
}

pub trait VideoEncoder: Send {
    fn configure(&mut self, config: &MediaConfig) -> MediaResult<()>;
    fn encode(&mut self, frame: CapturedFrame) -> MediaResult<Option<EncodedFrame>>;
    fn request_keyframe(&mut self) -> MediaResult<()>;
}

pub trait VideoDecoder: Send {
    fn configure(&mut self, config: &MediaConfig) -> MediaResult<()>;
    fn decode(&mut self, frame: EncodedFrame) -> MediaResult<Option<DecodedFrame>>;
}

pub trait FramePresenter: Send {
    fn attach(&mut self, target: PresentationTarget) -> MediaResult<()>;
    fn present(&mut self, frame: DecodedFrame) -> MediaResult<PresentationStamp>;
    fn detach(&mut self);
}
```

On non-Windows platforms, `VideoSurface` contains only a test/simulated variant. Windows D3D types remain behind `cfg(windows)` and private wrappers.

**Step 4: Run crate and workspace checks**

Run:

```powershell
cargo test -p rshare-media
cargo check --workspace
```

Expected: all media tests PASS and the workspace compiles.

**Step 5: Commit**

```powershell
git add Cargo.toml crates/rshare-media
git commit -m "feat: scaffold bounded media pipeline"
```

### Task 22: Bind Media Authorization to the Authenticated Control Generation

**Files:**
- Create: `crates/rshare-core/src/media.rs`
- Modify: `crates/rshare-core/src/lib.rs`
- Modify: `crates/rshare-core/src/protocol/mod.rs`
- Create: `apps/rshare-daemon/src/media_session.rs`
- Modify: `apps/rshare-daemon/Cargo.toml`
- Modify: `apps/rshare-daemon/src/main.rs`
- Create: `crates/rshare-core/tests/media_contract.rs`
- Test: `apps/rshare-daemon/src/media_session.rs`

**Step 1: Write failing identity and token tests**

Require:

- media token is single-use, expires, and is bound to device id, certificate fingerprint, control connection generation, session id, and display id;
- revoking/closing the control generation invalidates unused media tokens and live sessions.
- stored grants and logs never contain the plaintext token.
- a delayed close for an old control generation cannot revoke a replacement generation's token/session.

```rust
#[test]
fn token_cannot_be_replayed_or_used_for_another_display() {
    let clock = FakeMediaClock::at_millis(100_000);
    let mut registry = MediaAuthorizationRegistry::new(clock);
    let grant = registry.issue(bound_request(DISPLAY_A), Duration::from_secs(10));
    assert!(registry.consume(&grant.token, bound_attempt(DISPLAY_B)).is_err());
    assert!(registry.consume(&grant.token, bound_attempt(DISPLAY_A)).is_ok());
    assert!(matches!(
        registry.consume(&grant.token, bound_attempt(DISPLAY_A)),
        Err(MediaAuthorizationError::AlreadyUsed)
    ));
}
```

The registry test above lives in `apps/rshare-daemon/src/media_session.rs`; `crates/rshare-core/tests/media_contract.rs` tests only serialization, redacted formatting, and request/offer/end-reason compatibility so core never depends on the daemon or network crate.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-core --test media_contract
cargo test -p rshare-daemon media_session::tests
```

Expected: FAIL because media grants do not exist.

**Step 3: Add the bound grant types**

Keep request/offer/end-reason wire types and the wire token in `rshare-core`:

```rust
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MediaOneTimeToken([u8; 32]);

impl std::fmt::Debug for MediaOneTimeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MediaOneTimeToken([REDACTED])")
    }
}
```

Serialize it with one canonical URL-safe encoding and reject any decoded value that is not exactly 32 bytes. Put the registry/context in daemon `media_session.rs`, where `PeerAuthContext` from `rshare-net` is already available; do not add a core→net dependency:

```rust
pub struct MediaGrantContext {
    pub peer: PeerAuthContext,
    pub media_session_id: Uuid,
    pub display_id: String,
}

pub struct MediaAuthorizationRegistry<C: MediaClock> {
    clock: C,
    grants_by_hash: HashMap<[u8; 32], StoredMediaGrant>,
    used_tombstones: BoundedExpiryMap<[u8; 32], Instant>,
}
```

Generate the public token as 32 cryptographically random bytes from the OS CSPRNG, return it once, and store only its SHA-256 hash. `consume` atomically validates token hash, expiry, peer id, certificate fingerprint, control connection id, session id, and display id before moving the hash into a bounded expiry tombstone so replay returns `AlreadyUsed` without retaining plaintext. Revoking uses compare-by-`ControlConnectionId`: an old generation cannot revoke its replacement. Under registry/session locks, remove grants and collect live session handles only; release every lock before awaiting stop/close.

**Step 4: Add negotiation messages and a one-time registry**

Add control-only protocol messages:

```rust
Message::StartMediaSession { request: StartMediaSessionRequest }
Message::MediaSessionOffer { offer: MediaSessionOffer }
Message::MediaSessionStop { session_id: Uuid, reason: String }
Message::MediaSessionState { state: MediaSessionState }
```

`MediaSessionOffer` includes endpoint, expiry, and the one-time token. Video bytes and keyframe/congestion feedback never become a general `Message` variant; Task 24 carries those over the media connection's own reliable control stream.

`MediaSessionManager` accepts only a selected currently-present physical/IDD display and publishes state through `StateAggregator`. It returns `Unsupported` before issuing a token unless both authenticated peers negotiated the same nonzero optional `separate_media_quic_version`.

**Step 5: Run security and protocol tests**

Run:

```powershell
cargo test -p rshare-core media
cargo test -p rshare-daemon media_session
```

Expected: all tests PASS, including one-time, expiry, binding, revocation, and plaintext-redaction checks.

**Step 6: Commit**

```powershell
git add crates/rshare-core/src/media.rs crates/rshare-core/src/lib.rs crates/rshare-core/src/protocol/mod.rs crates/rshare-core/tests/media_contract.rs apps/rshare-daemon/Cargo.toml apps/rshare-daemon/src/media_session.rs apps/rshare-daemon/src/main.rs
git commit -m "feat: authorize media with bound peer identity"
```

### Task 23: Add Bounded Video Packetization, Reassembly, and Deadlines

**Files:**
- Create: `crates/rshare-media/src/packet.rs`
- Create: `crates/rshare-media/src/reassembly.rs`
- Modify: `crates/rshare-media/src/lib.rs`
- Test: `crates/rshare-media/src/packet.rs`
- Test: `crates/rshare-media/src/reassembly.rs`

**Step 1: Write failing packet/reassembly tests**

Cover:

- packet payload respects negotiated datagram limit including headers;
- reordered fragments reassemble one frame;
- duplicate fragments do not increase memory;
- incomplete frame expires at deadline and requests a keyframe if needed;
- frame count, total bytes, and per-frame fragment count have hard bounds;
- overload discards oldest delta frame, not the newest complete frame;
- wrong session id and impossible packet indexes fail closed.

```rust
#[test]
fn missing_fragment_expires_without_unbounded_state() {
    let mut reassembly = ReassemblyBuffer::new(ReassemblyLimits {
        max_frames: 3,
        max_bytes: 8 * 1024 * 1024,
        frame_deadline: Duration::from_millis(20),
    });
    for frame_id in 0..10_000 {
        reassembly.push(first_fragment_only(frame_id), time_at(frame_id)).unwrap();
    }
    let outcome = reassembly.expire(time_at(10_100));
    assert!(reassembly.frames() <= 3);
    assert!(reassembly.bytes() <= 8 * 1024 * 1024);
    assert!(outcome.dropped_incomplete > 0);
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media packet
cargo test -p rshare-media reassembly
```

Expected: FAIL because packetizer/reassembler do not exist.

**Step 3: Implement the packet format**

Use:

```rust
pub struct VideoPacket {
    pub protocol_version: u8,
    pub session_id: MediaSessionId,
    pub frame_id: u64,
    pub packet_index: u16,
    pub packet_count: u16,
    pub pts_us: u64,
    pub flags: VideoPacketFlags,
    pub payload: bytes::Bytes,
}
```

Packetizer computes `max_payload = max_datagram_size - encoded_header_len`, rejects more than `u16::MAX` fragments, and makes no full-frame copy beyond the encoder output.

`ReassemblyBuffer` uses a bounded map keyed by frame id, tracks a bitset and exact bytes, expires by monotonic deadline, and consistently returns:

```rust
pub struct ReassemblyReport {
    pub completed: Option<EncodedFrame>,
    pub action: ReassemblyAction,
    pub dropped_incomplete: u64,
    pub dropped_bytes: u64,
}

pub enum ReassemblyAction {
    None,
    Pending,
    Dropped,
    RequestKeyframe,
}
```

Both `push` and `expire` return this report; the test's `outcome.dropped_incomplete` therefore measures explicit bounded-state eviction rather than relying on an incompatible enum shape.

**Step 4: Run media tests**

Run:

```powershell
cargo test -p rshare-media
```

Expected: all tests PASS, including 10,000 incomplete-frame stress.

**Step 5: Commit**

```powershell
git add crates/rshare-media/src/packet.rs crates/rshare-media/src/reassembly.rs crates/rshare-media/src/lib.rs
git commit -m "feat: bound video packet reassembly"
```

### Task 24: Add a Separate Media QUIC Connection and Prove Control Isolation

**Files:**
- Create: `crates/rshare-media/src/transport.rs`
- Modify: `crates/rshare-media/src/lib.rs`
- Modify: `crates/rshare-media/Cargo.toml`
- Modify: `crates/rshare-net/src/encryption.rs`
- Modify: `crates/rshare-net/src/lib.rs`
- Create: `crates/rshare-net/tests/control_media_isolation.rs`
- Modify: `crates/rshare-core/src/config.rs`
- Modify: `crates/rshare-core/src/ipc.rs`
- Modify: `crates/rshare-core/src/daemon_client.rs`
- Modify: `crates/rshare-platform/src/firewall.rs`
- Modify: `apps/rshare-cli/src/config.rs`
- Modify: `apps/rshare-daemon/src/lib.rs`
- Modify: `apps/rshare-daemon/src/media_session.rs`
- Modify: `apps/rshare-daemon/src/main.rs`
- Modify: `apps/rshare-desktop/src-tauri/Cargo.toml`
- Create: `apps/rshare-desktop/src-tauri/src/media_receiver.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Test: `crates/rshare-media/src/transport.rs`
- Test: `crates/rshare-net/tests/control_media_isolation.rs`
- Test: `apps/rshare-desktop/src-tauri/src/media_receiver.rs`

**Step 1: Write failing media transport tests**

Require:

- media opens a second QUIC endpoint/connection, not a stream on control;
- certificate identity and one-time token both validate before accepting video;
- authentication timeout/rejection consumes no video and leaves no session;
- 0-RTT/early video is disabled, and datagrams received before `Accepted` are discarded without allocation;
- encoded-frame send queue has configured frame/byte bounds;
- congestion drops stale delta frames and requests keyframes;
- stopping/overloading media leaves control handle and queue metrics unchanged.

```rust
#[tokio::test]
#[ignore = "fixed-runner real-time control/media isolation"]
async fn saturated_media_connection_does_not_change_control_latency_or_depth() {
    let fixture = ControlAndMediaFixture::new().await;
    fixture.media.block_datagram_writes();
    fixture.media.push_frames(10_000);
    fixture.control.try_send_reliable_input(reliable_key_down()).unwrap();
    assert!(fixture.control.received_within(Duration::from_millis(20)).await);
    assert_eq!(fixture.control.max_queue_depth(), 1);
    assert!(fixture.media.queue_depth() <= fixture.media.capacity());
}
```

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media saturated_media_connection
```

Expected: FAIL because no media transport exists.

**Step 3: Implement the independent connection**

`MediaTransport` owns its own Quinn `Endpoint`, `Connection`, congestion state, datagram send/receive loops, packetizer, and reassembler. `rshare-media` depends directly on Quinn/rustls and `rshare-core`, but not on `rshare-net` or Tauri. `rshare-net` publicly exposes certificate/trust builders returning Quinn/rustls configuration. The source daemon passes the server config into `MediaTransport`; the receiving desktop depends on `rshare-net` only to load its existing local device identity and build a client config pinned to the already-authenticated peer fingerprint. This avoids a crate cycle and prevents the desktop from starting a second, independent TOFU decision.

Media uses ALPN `rshare-media/1`, mandatory client certificates, and the same trusted fingerprints as the authenticated control peers. Disable 0-RTT. Before session registration or any video datagram allocation, require one bounded reliable media-control stream:

```rust
MediaClientHello {
    version: 1,
    session_id: Uuid,
    token: MediaOneTimeToken,
}

MediaServerHello::Accepted { session_id: Uuid }
MediaServerHello::Rejected { reason: MediaAuthRejectReason }
```

The server enforces a five-second authentication timeout and passes the TLS client certificate fingerprint plus the hello to a daemon-provided `MediaAuthorizer` trait/closure. The daemon atomically consumes a grant bound to peer id, fingerprint, `ControlConnectionId`, session id, and display id before replying `Accepted`. Until then, do not start capture/encode, do not create reassembly/session state, and discard any datagram without buffering. The client pins the server fingerprint supplied by its local daemon from the authenticated control connection; it never re-TOFUs. Media must not share:

- control connection object;
- control writer channels;
- control flow/congestion queues;
- daemon `NetworkManager` mutex;
- control connection idle/error lifecycle.

Apply a token bucket capped by `MediaConfig::max_bitrate_bps`. The sender queue holds no more than two encoded frames and prefers the newest. Receiver jitter holds approximately one frame and presents the newest complete decodable frame.

Add serde-defaulted `features.media_streaming = false` and `network.media_port = 27433`. Open the media UDP firewall rule only when enabled. A matching `ControlConnectionId` close/replacement stops media immediately; media failure does not close control.

Process ownership is explicit:

- source daemon owns capture, encode, and media server;
- receiving Tauri process owns media client, reassembly, decode, and presentation so D3D textures never cross a process boundary;
- receiving local daemon negotiates `StartMediaSession` over authenticated control and returns only the offer metadata/token plus the authenticated remote certificate fingerprint through framed local IPC;
- Tauri loads the same local device certificate/trust identity, consumes the offer on the media endpoint, and never routes H.264/video packets through daemon IPC or a Tauri event.

Add local IPC `StartDisplayStream { peer_id, display_id }`/`StopDisplayStream { session_id }`. Expose public `daemon_client::request_start_display_stream` and `request_stop_display_stream`; do not require Tauri to call the client's private generic sender. The start command awaits the correlated peer offer with a bounded timeout and returns metadata only. Wire `media_receiver` from desktop `main.rs`: request offer through local framed IPC, build the pinned media client TLS config, authenticate, then start receive/reassembly. Token values are redacted from all logs/errors.

When a control generation closes/replaces, remove grants and live handles under a short lock, release the lock, then await media stop/QUIC close. Media failure follows the same no-lock-across-await rule and never closes control.

**Step 4: Run media tests plus the combined stress harness**

Run:

```powershell
cargo test -p rshare-media transport
cargo test -p rshare-net --test control_media_isolation saturated_media_connection_does_not_change_control_latency_or_depth -- --exact --ignored --nocapture
cargo test -p rshare-media saturated_media_connection -- --nocapture
cargo test -p rshare-desktop media_receiver
```

The fixed-runner wrapper also parses Cargo's summary and fails if exactly one named ignored test did not run. Expected:

- all correctness tests PASS;
- media saturation adds no more than 5 ms to control p99;
- media/control queue bounds are never exceeded.

**Step 5: Commit**

```powershell
git add crates/rshare-media/Cargo.toml crates/rshare-media/src/transport.rs crates/rshare-media/src/lib.rs crates/rshare-net/src/encryption.rs crates/rshare-net/src/lib.rs crates/rshare-net/tests/control_media_isolation.rs crates/rshare-core/src/config.rs crates/rshare-core/src/ipc.rs crates/rshare-core/src/daemon_client.rs crates/rshare-platform/src/firewall.rs apps/rshare-cli/src/config.rs apps/rshare-daemon/src/lib.rs apps/rshare-daemon/src/media_session.rs apps/rshare-daemon/src/main.rs apps/rshare-desktop/src-tauri/Cargo.toml apps/rshare-desktop/src-tauri/src/media_receiver.rs apps/rshare-desktop/src-tauri/src/main.rs
git commit -m "feat: isolate media on a separate quic connection"
```

### Task 25: Implement Windows GPU Capture with WGC and DXGI Fallback

**Files:**
- Modify: `crates/rshare-media/Cargo.toml`
- Modify: `crates/rshare-media/src/lib.rs`
- Create: `crates/rshare-media/src/windows/mod.rs`
- Create: `crates/rshare-media/src/windows/d3d.rs`
- Create: `crates/rshare-media/src/windows/capture.rs`
- Create: `crates/rshare-media/src/windows/wgc.rs`
- Create: `crates/rshare-media/src/windows/dxgi.rs`
- Modify: `crates/rshare-platform/src/lib.rs`
- Modify: `crates/rshare-platform/src/display.rs`
- Modify: `crates/rshare-platform/src/windows.rs`
- Create: `crates/rshare-platform/tests/windows_display_resolver.rs`
- Create: `crates/rshare-media/tests/windows_capture_smoke.rs`
- Test: `crates/rshare-media/src/windows/capture.rs`

**Step 1: Write failing capture-selection and lifecycle tests**

Use mock WGC/DXGI factories to prove:

- selected display id maps to exactly one monitor/IDD source;
- an unknown, duplicate, or stale display id fails closed rather than selecting a default monitor;
- WGC is preferred;
- access-denied/unsupported WGC falls back to DXGI;
- display removal or device loss ends capture with a truthful reason;
- queue holds at most two GPU frames;
- no CPU readback method is called on the normal path.

```rust
#[test]
fn wgc_failure_falls_back_to_dxgi_for_same_display() {
    let factories = factories(
        failing_wgc(MediaError::Unsupported),
        working_dxgi(DISPLAY_ID),
    );
    let source = WindowsFrameSource::open(DISPLAY_ID, factories).unwrap();
    assert_eq!(source.backend(), CaptureBackendKind::DxgiDuplication);
}
```

In `windows_capture_smoke.rs`, use `#![cfg(windows)]` and mark the exact hardware test `captures_selected_display_1080p_without_cpu_readback` with `#[ignore = "requires unlocked fixed Windows GPU runner"]`.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media windows::capture::tests
```

Expected: FAIL because the Windows modules do not exist.

**Step 3: Add shared D3D11 device and WGC source**

Enable only required `windows` crate features for:

- Win32 Graphics Direct3D/Direct3D11/Dxgi;
- Windows.Graphics.Capture;
- Windows.Graphics.DirectX.Direct3D11;
- Win32 System WinRT/Com;
- monitor/display enumeration.

Keep Windows handles at crate boundaries as raw wrappers so Tauri and `rshare-media` do not leak potentially different `windows` crate types:

```rust
pub struct NativeHwnd(pub isize);
pub struct NativeMonitorHandle {
    pub hmonitor: isize,
    pub adapter_luid: u64,
    pub output_index: u32,
}
```

Use the canonical display inventory/ID algorithm in `rshare-platform`; do not independently enumerate and invent IDs in `rshare-media`. Add and test:

```rust
pub fn resolve_windows_display_handle(
    display_id: &str,
) -> Result<NativeDisplayHandle, DisplayResolveError>;

pub struct NativeDisplayHandle {
    pub hmonitor: isize,
    pub adapter_luid: u64,
    pub output_index: u32,
}
```

`rshare-media` takes a Windows-target-only dependency on `rshare-platform` and converts this raw wrapper immediately into its private `NativeMonitorHandle`. Resolver tests use the same fixtures as display enumeration and cover physical, IDD, mixed-resolution/negative-coordinate, removal, and duplicate-ID cases.

`D3dDeviceContext` creates one hardware D3D11 device with video support and multithread protection. WGC:

1. resolves monitor from stable `display_id`;
2. creates `GraphicsCaptureItem` through monitor interop without a picker;
3. creates a free-threaded D3D11 frame pool with two buffers;
4. copies the borrowed WGC surface into an owned D3D11 texture before closing the capture frame;
5. recreates the pool on size change;
6. publishes to a one-frame latest queue;
7. stops on closed item/session/device loss.

**Step 4: Add DXGI Desktop Duplication fallback**

Select the matching `IDXGIOutput1`, call `DuplicateOutput`, acquire with a frame deadline, copy into an owned GPU texture, and always call `ReleaseFrame` even after copy failure. Handle `DXGI_ERROR_WAIT_TIMEOUT`, bounded `DXGI_ERROR_ACCESS_LOST` recreation, rotation, and size changes without copying to CPU.

Expose both through `WindowsFrameSource: FrameSource`.

**Step 5: Run unit and Windows smoke tests**

Run:

```powershell
cargo test -p rshare-media windows::capture
cargo test -p rshare-media --test windows_capture_smoke captures_selected_display_1080p_without_cpu_readback -- --exact --ignored --nocapture
```

Expected: unit tests PASS. On a Windows machine with an unlocked desktop, the ignored smoke test captures 600 frames from the selected display, reports ≥55 fps at 1080p, and records zero CPU readbacks. If hardware is unavailable, save the exact capability/error and do not claim the capture gate.

**Step 6: Commit**

```powershell
git add crates/rshare-media/Cargo.toml crates/rshare-media/src/lib.rs crates/rshare-media/src/windows crates/rshare-media/tests/windows_capture_smoke.rs crates/rshare-platform/src/lib.rs crates/rshare-platform/src/display.rs crates/rshare-platform/src/windows.rs crates/rshare-platform/tests/windows_display_resolver.rs
git commit -m "feat: capture windows displays on d3d11"
```

### Task 26: Add GPU NV12 Conversion and Media Foundation H.264 Encode/Decode

**Files:**
- Create: `crates/rshare-media/src/windows/nv12.rs`
- Create: `crates/rshare-media/src/windows/mf_encoder.rs`
- Create: `crates/rshare-media/src/windows/mf_decoder.rs`
- Create: `crates/rshare-media/tests/windows_codec_smoke.rs`
- Modify: `crates/rshare-media/src/windows/mod.rs`
- Modify: `crates/rshare-media/Cargo.toml`
- Modify: `apps/rshare-daemon/src/media_session.rs`
- Test: `crates/rshare-media/src/windows/mf_encoder.rs`
- Test: `crates/rshare-media/src/windows/mf_decoder.rs`

**Step 1: Write failing codec configuration tests**

Mock the Media Foundation transform/attributes seam and require:

- hardware-aware transform selection;
- H.264, 1920x1080, 60 fps, 8-20 Mbps;
- low-latency mode enabled;
- no B frames and no lookahead;
- shallow input/output queues;
- regular keyframes and explicit keyframe request;
- decoder output remains a D3D11 texture;
- software-only transforms are rejected as unavailable for the MVP rather than silently consuming CPU.

In `windows_codec_smoke.rs`, use `#![cfg(windows)]` and mark `hardware_h264_1080p60_round_trip` with `#[ignore = "requires fixed Windows hardware codec runner"]`.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media mf_encoder
cargo test -p rshare-media mf_decoder
```

Expected: FAIL because the Windows codec modules do not exist.

**Step 3: Implement GPU conversion and encoder**

Create NV12 textures in the shared D3D11 device. Use the D3D11 video processor for BGRA→NV12 scaling/conversion; no staging texture/readback on the normal path.

Initialize Media Foundation once per process. Enumerate hardware H.264 encoders using `MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER`, attach `IMFDXGIDeviceManager`, and set low-latency attributes/CODECAPI values. Feed D3D11-backed samples and drain output without building a queue deeper than two frames.

**Step 4: Implement decoder**

Select a hardware H.264 decoder, attach the same device-manager model, request D3D11-aware NV12/BGRA output, handle stream changes, and emit `DecodedFrame` textures without CPU conversion.

Wire the source-daemon session worker as:

```text
WindowsFrameSource -> capacity-2 newest GPU queue -> NV12 converter
-> hardware encoder -> capacity-2 newest encoded queue -> MediaTransport server
```

Cancellation/display removal/device loss closes all stages in order and publishes one media end reason; no worker keeps an `Arc` that prevents session teardown.

**Step 5: Run unit and hardware smoke tests**

Run:

```powershell
cargo test -p rshare-media mf_
cargo test -p rshare-media --test windows_codec_smoke hardware_h264_1080p60_round_trip -- --exact --ignored --nocapture
```

Expected: unit tests PASS. Hardware smoke test encodes/decodes at least 3,600 1080p frames, average throughput ≥60 fps, queue depth ≤2, and no CPU readbacks. It writes capture/convert/encode/decode histograms.

**Step 6: Commit**

```powershell
git add crates/rshare-media/src/windows/nv12.rs crates/rshare-media/src/windows/mf_encoder.rs crates/rshare-media/src/windows/mf_decoder.rs crates/rshare-media/src/windows/mod.rs crates/rshare-media/Cargo.toml crates/rshare-media/tests/windows_codec_smoke.rs apps/rshare-daemon/src/media_session.rs
git commit -m "feat: add hardware h264 media pipeline"
```

### Task 27: Present Decoded Frames in a Native D3D11 Tauri Child Surface

**Files:**
- Create: `crates/rshare-media/src/windows/presenter.rs`
- Modify: `crates/rshare-media/src/windows/mod.rs`
- Create: `apps/rshare-desktop/src-tauri/src/media_runtime.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/media_receiver.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/main.rs`
- Create: `apps/rshare-desktop-frontend/src/app/media-surface.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/media-surface.test.mjs`
- Create: `apps/rshare-desktop-frontend/src/app/components/RemoteDisplaySurface.tsx`
- Modify: `apps/rshare-desktop-frontend/src/app/App.tsx`
- Test: `crates/rshare-media/src/windows/presenter.rs`
- Test: `apps/rshare-desktop/src-tauri/src/media_runtime.rs`

**Step 1: Write failing presenter lifecycle tests**

With a fake HWND/swap chain seam, prove:

- attach creates one child surface and a two-buffer flip-model swap chain;
- resize reuses the child window and releases old back buffers;
- newest decoded texture is copied/rendered directly to the swap chain;
- detach/device loss releases D3D/Win32 resources exactly once;
- hiding/minimizing suspends presentation and does not accumulate frames.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media presenter
cargo test -p rshare-desktop media_runtime
```

Expected: FAIL because presenter/runtime do not exist.

**Step 3: Implement the native presenter**

Obtain `WebviewWindow::hwnd()` and immediately convert it to `NativeHwnd(pub isize)` before crossing into `rshare-media`. Create a `WS_CHILD | WS_VISIBLE` child window attached to the Tauri webview window. Use:

- `DXGI_SWAP_EFFECT_FLIP_DISCARD`;
- two buffers;
- frame-latency waitable object with maximum latency 1;
- D3D11 video processor or shader path for NV12/BGRA texture presentation;
- resize/dpi handlers driven by Tauri window events.

Expose Tauri commands:

```rust
start_media_view(session_id, display_id, bounds) -> MediaViewState
resize_media_view(session_id, bounds) -> ()
stop_media_view(session_id) -> ()
```

`RemoteDisplaySurface` reserves layout space and sends bounds/lifecycle commands; it does not receive video bytes or draw video into DOM/canvas. A `ResizeObserver` converts CSS bounds to physical pixels with current device-pixel ratio and sends at most one bounds update per RAF. Unmount/window close destroys the child surface idempotently.

The receiving desktop runtime owns this same-process pipeline:

```text
MediaTransport client -> bounded reassembly/latest encoded frame
-> hardware decoder -> one-frame latest decoded queue -> D3D11 presenter
```

The decoder and presenter share a D3D11 device/device manager where supported. No H.264 payload or decoded texture enters React state, a Tauri event, or daemon IPC.

**Step 4: Run Rust/frontend tests and build**

Run:

```powershell
cargo test -p rshare-media presenter
cargo test -p rshare-desktop media_runtime
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
```

Expected: all tests/build PASS.

**Step 5: Commit**

```powershell
git add crates/rshare-media/src/windows/presenter.rs crates/rshare-media/src/windows/mod.rs apps/rshare-desktop/src-tauri/src/media_runtime.rs apps/rshare-desktop/src-tauri/src/media_receiver.rs apps/rshare-desktop/src-tauri/src/main.rs apps/rshare-desktop-frontend/src/app/media-surface.mjs apps/rshare-desktop-frontend/src/app/media-surface.test.mjs apps/rshare-desktop-frontend/src/app/components/RemoteDisplaySurface.tsx apps/rshare-desktop-frontend/src/app/App.tsx
git commit -m "feat: present remote video on native d3d11 surface"
```

### Task 28: Add Cursor Overlay, Adaptive Bitrate, and Fail-Safe Media Recovery

**Files:**
- Create: `crates/rshare-media/src/adaptation.rs`
- Modify: `crates/rshare-media/src/lib.rs`
- Modify: `crates/rshare-media/src/types.rs`
- Modify: `crates/rshare-media/src/queue.rs`
- Modify: `crates/rshare-media/src/transport.rs`
- Modify: `crates/rshare-media/src/reassembly.rs`
- Modify: `crates/rshare-media/src/windows/mf_encoder.rs`
- Modify: `crates/rshare-media/src/windows/presenter.rs`
- Modify: `apps/rshare-daemon/src/media_session.rs`
- Modify: `apps/rshare-daemon/src/input_runtime.rs`
- Modify: `apps/rshare-daemon/src/state_aggregator.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/ui_state_bridge.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/media_receiver.rs`
- Modify: `apps/rshare-desktop/src-tauri/src/media_runtime.rs`
- Test: `crates/rshare-media/src/adaptation.rs`
- Test: `apps/rshare-daemon/src/media_session.rs`

**Step 1: Write failing cursor/adaptation/recovery tests**

Require:

- cursor realtime state reaches overlay without waiting for video;
- reliable click anchor places overlay at click coordinates;
- increasing queue age first lowers bitrate, then drops old delta frames, then reduces resolution;
- recovery increases bitrate slowly and within configured cap;
- receiver feedback reaches the source controller over the media reliable-control stream;
- bitrate action changes the live hardware encoder;
- resolution change flushes the old generation, announces a reliable format change, and resumes only from a new-generation keyframe;
- lock/session change, display removal, token revocation, capture/codec failure, or control identity loss ends media with the exact reason;
- media failure does not suspend a healthy input session;
- input failure may stop/release input without corrupting media state.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media adaptation
cargo test -p rshare-daemon media_session
```

Expected: FAIL because cursor overlay/adaptation/recovery policy does not exist.

**Step 3: Implement deterministic adaptation**

Implement a pure controller:

```rust
pub struct AdaptationController {
    min_bitrate_bps: u32,
    max_bitrate_bps: u32,
    current_bitrate_bps: u32,
    resolution_scale: ResolutionScale,
}

pub enum AdaptationAction {
    Keep,
    SetBitrate(u32),
    DropExpiredDeltaFrames,
    SetResolution(ResolutionScale),
    RequestKeyframe,
}
```

Inputs are smoothed RTT, send failures, queue age, packet/frame loss, and decode deadline misses. Never use control-lane queue depth as a media control signal.

Define closed media-control messages on the media connection:

```rust
pub enum MediaControlMessage {
    Feedback(MediaFeedback),
    RequestKeyframe { format_generation: u32 },
    FormatChanged(MediaFormatChanged),
    CursorShape(CursorShapeUpdate),
}

pub struct MediaFeedback {
    pub latest_presented_frame: u64,
    pub packet_loss_ppm: u32,
    pub frame_loss_ppm: u32,
    pub decode_deadline_misses: u32,
    pub presentation_age_us: u64,
}

pub struct MediaTransportStats {
    pub smoothed_rtt_us: u64,
    pub sender_queue_age_us: u64,
    pub send_failures: u64,
}

pub struct MediaFormatChanged {
    pub format_generation: u32,
    pub config: MediaConfig,
    pub first_frame_is_keyframe: bool,
}
```

The receiving `media_receiver` derives only receiver-observable feedback from reassembly/decode/present counters and sends it on the media reliable-control stream. It never fabricates source queue age. The source `MediaTransport` computes RTT from its own media-control ping/ack sequence and samples its own send failures/queue age into `MediaTransportStats`; `MediaSessionManager` combines those local stats with remote feedback before feeding `AdaptationController`. `SetBitrate` invokes `MfH264Encoder::set_bitrate`; `DropExpiredDeltaFrames` purges expired delta frames from the bounded encoded queue/transport; `RequestKeyframe` calls the encoder.

`SetResolution` is a generation barrier: flush old capture/convert/encode output, increment `format_generation`, reconfigure the GPU pipeline, send `MediaFormatChanged` reliably, then emit a keyframe tagged with that generation. The receiver flushes old reassembly/decoder state, rejects datagrams for an unknown/old generation, reconfigures, and presents only after the announced generation's keyframe.

Render cursor shape/position as a native presenter overlay without putting it in React state. Source cursor shape/in-frame capability travels on `MediaControlMessage::CursorShape`; receiver-side realtime position and reliable click anchors come from `InputRuntime` through the authoritative local UI stream. `ui_state_bridge` routes these updates directly into `media_runtime`, which calls the presenter overlay API. When source capture supports cursor exclusion, exclude it; otherwise mark cursor-in-frame and disable the duplicate overlay. Tests prove a realtime update reaches the presenter even while video datagrams are blocked and a reliable click anchor is applied before a later pointer sample.

**Step 4: Add failure lifecycle hooks**

Subscribe `MediaSessionManager` to authenticated control-generation changes, Windows session lock/unlock, display inventory, and pipeline failures. Stop and revoke immediately. Publish `UiMediaSessionState::{Starting, Streaming, Degraded, Stopped { reason }}`.

**Step 5: Run tests**

Run:

```powershell
cargo test -p rshare-media
cargo test -p rshare-daemon media_session
cargo test -p rshare-daemon input_runtime
```

Expected: all tests PASS; media failure tests leave control active, and control identity loss closes media.

**Step 6: Commit**

```powershell
git add crates/rshare-media/src/adaptation.rs crates/rshare-media/src/lib.rs crates/rshare-media/src/types.rs crates/rshare-media/src/queue.rs crates/rshare-media/src/transport.rs crates/rshare-media/src/reassembly.rs crates/rshare-media/src/windows/mf_encoder.rs crates/rshare-media/src/windows/presenter.rs apps/rshare-daemon/src/media_session.rs apps/rshare-daemon/src/input_runtime.rs apps/rshare-daemon/src/state_aggregator.rs apps/rshare-desktop/src-tauri/src/ui_state_bridge.rs apps/rshare-desktop/src-tauri/src/media_receiver.rs apps/rshare-desktop/src-tauri/src/media_runtime.rs
git commit -m "feat: recover and adapt low latency media"
```

### Task 29: Add Automated Media and Combined-Load Performance Gates

**Files:**
- Create: `crates/rshare-media/benches/media_pipeline.rs`
- Create: `crates/rshare-media/tests/media_load.rs`
- Create: `apps/rshare-desktop-frontend/tests/performance/media-view.spec.mjs`
- Create: `scripts/perf/run-media.ps1`
- Modify: `.github/workflows/performance-nightly-windows.yml`
- Create: `docs/performance/media-windows-validation.md`
- Modify: `crates/rshare-media/Cargo.toml`
- Modify: `crates/rshare-net/tests/control_media_isolation.rs`
- Modify: `tools/rshare-perf/Cargo.toml`
- Create: `tools/rshare-perf/src/media.rs`
- Modify: `tools/rshare-perf/src/main.rs`

**Step 1: Write failing simulated combined-load test**

Run simulated 1080p60 encoded frames for 30 minutes of virtual time alongside modeled 1000 Hz mouse and 100 Hz reliable input. Assert deterministic correctness only:

- media queues stay bounded;
- incomplete/expired frames are never presented;
- latest complete frame wins;
- zero reliable input loss/duplication;
- no held control state remains at shutdown.

Paused Tokio time must not produce or gate performance percentiles. Use it only for lifecycle, queue, ordering, timeout, and cleanup invariants. Put the real control+media concurrent latency measurement in `rshare-perf media` plus the actual daemon/net integration harness; only that real-time fixed-runner evidence can enforce media load adds ≤5 ms to control p99.

Add `media_not_run_cannot_pass_acceptance`: `Unsupported` or `NotRun` must remain a non-pass verdict.

**Step 2: Run and verify failure**

Run:

```powershell
cargo test -p rshare-media --test media_load
```

Expected: FAIL until the load fixture/reporting is added.

**Step 3: Add fixed-runner media harnesses**

Add Criterion as a `rshare-media` dev dependency and register the executable benchmark in `crates/rshare-media/Cargo.toml`:

```toml
[[bench]]
name = "media_pipeline"
harness = false
```

`run-media.ps1` performs one exactly-five-run same-config batch of:

- WGC physical display capture;
- IDD display capture if installed;
- GPU convert/encode/decode/present;
- media QUIC with configurable 0/0.1/1% loss and 0/5/20 ms jitter;
- concurrent 1000 Hz control and UI state stream;
- 30-minute soak.

Collect stage histograms for capture, convert, encode, packetize, wire, reassembly, decode, present, control-without-media, control-with-media, and glass-to-glass proxy. Upload every raw JSON, scenario parameters/seed, build fingerprints, hardware/driver inventory, codec selection, and queue/drop metrics.

`performance-nightly-windows.yml`:

- compiles all benches on hosted Windows;
- runs deterministic simulated tests on hosted runners;
- runs strict hardware scenarios only on a labeled fixed Windows self-hosted runner;
- resolves a reviewed baseline by id through the hash-verified manifest from Task 2 and compares exactly-five-run medians (>10%) and p95/p99 (>15%).

If CV exceeds 10%, preserve the full batch and rerun the entire five-run batch once; never replace individual runs. A second unstable batch fails.

**Step 4: Run deterministic and available hardware tests**

Run:

```powershell
cargo test -p rshare-media --locked
cargo test -p rshare-net --test control_media_isolation --locked saturated_media_connection_does_not_change_control_latency_or_depth -- --exact --ignored --nocapture
cargo bench -p rshare-media --bench media_pipeline --no-run --locked
cargo bench -p rshare-media --bench media_pipeline --locked -- --test
powershell -ExecutionPolicy Bypass -File scripts/perf/run-media.ps1 -OutputDirectory C:\tmp\rshare-media-results -SkipHardware:$false
```

Expected:

- deterministic suites PASS everywhere;
- fixed compatible hardware sustains 1080p60;
- all media queues stay bounded;
- control p99 increase ≤5 ms.

If hardware prerequisites are unavailable, rerun with `-SkipHardware`, mark the hardware gate pending, and do not claim 1080p60 or glass-to-glass latency.

**Step 5: Commit**

```powershell
git add crates/rshare-media/benches crates/rshare-media/tests/media_load.rs crates/rshare-media/Cargo.toml crates/rshare-net/tests/control_media_isolation.rs tools/rshare-perf/Cargo.toml tools/rshare-perf/src/media.rs tools/rshare-perf/src/main.rs apps/rshare-desktop-frontend/tests/performance/media-view.spec.mjs scripts/perf/run-media.ps1 .github/workflows/performance-nightly-windows.yml docs/performance/media-windows-validation.md
git commit -m "test: gate media latency and control isolation"
```

### Task 30: Run Dual-Machine Windows Acceptance and Record the Release Decision

**Files:**
- Modify: `tools/rshare-perf/src/dual.rs`
- Create: `scripts/perf/windows/Invoke-RSharePerfPreflight.ps1`
- Create: `scripts/perf/windows/Start-RSharePerfCapture.ps1`
- Create: `scripts/perf/windows/Merge-RSharePerfRun.ps1`
- Create: `scripts/perf/windows/rshare-control.wprp`
- Create: `docs/performance/2026-07-28-dual-machine-results.md`
- Modify: `docs/roadmap.md`
- Modify: `docs/performance/phase1-windows-validation.md`
- Modify: `docs/performance/media-windows-validation.md`

**Step 1: Write failing dual-report contract tests**

Add:

- `dual_reports_never_subtract_foreign_monotonic_clocks`;
- `dual_merge_requires_same_run_id_commit_and_protocol`;
- `dual_merge_enforces_zero_reliable_loss`;
- `dual_merge_rejects_missing_source_or_target_stages`;
- `release_default_rejects_perf_fault_injection`.

```powershell
cargo test -p rshare-perf dual
```

Expected: FAIL until dual arm/merge exists.

**Step 2: Implement local arm plus offline authenticated merge**

Do not open a new unauthenticated remote performance port. On each machine, `rshare-perf dual arm` talks only to its local authenticated daemon IPC, records a shared operator-provided run id, and writes one local JSON artifact. Copy artifacts through an operator-approved channel and merge offline.

The tools must:

- verify both peers report protocol v3, trusted identity, matching commit, Windows version, driver ABI, and hardware codec;
- establish warm adjacent control and one selected physical/IDD display;
- coordinate through request/ack sequence numbers, not wall-clock subtraction;
- collect local capture→send and remote receive→inject durations plus explicitly labeled RTT-derived end-to-end estimates;
- run the 660-second control matrix (idle, diagnostics, 10 MiB bulk, audio, media, slow-peer, 100 ms stall, reconnect, and lock/unlock) separately from the 1,800-second media/display-removal soak;
- collect UI event-to-paint and video capture-to-present metrics;
- never print media tokens or certificate private material.

The ordinary software artifacts never subtract foreign monotonic clocks. `estimated_capture_to_inject_us` may combine local stages and RTT for diagnosis only and can never satisfy an absolute SLO. A PASS for capture→inject/reliable end-to-end latency or jitter requires one of:

1. a common-clock external hardware input/output timing rig; or
2. a documented PTP/correlation setup with pre/post calibration, retained raw correlation evidence, and worst-case cross-machine uncertainty ≤0.5 ms.

Every absolute result records `measurement_method`, `uncertainty_us`, calibration artifact/checksum, and raw trace path. If neither method is available or uncertainty exceeds 0.5 ms, mark the software end-to-end gate `PENDING`; do not halve RTT or combine unsynchronized timestamps as proof.

Control commands, run separately on source and target:

```powershell
cargo run --release --locked -p rshare-perf -- dual arm --run-id <UUID> --role source --scenario control --duration-secs 660 --output C:\tmp\rshare-perf\source.json
cargo run --release --locked -p rshare-perf -- dual arm --run-id <UUID> --role target --scenario control --duration-secs 660 --output C:\tmp\rshare-perf\target.json
```

Offline merge:

```powershell
cargo run --release --locked -p rshare-perf -- dual merge --source C:\tmp\rshare-perf\source.json --target C:\tmp\rshare-perf\target.json --budget perf\budgets\windows-dual.toml --output C:\tmp\rshare-perf\combined.json
```

Exact 100 ms stalls are available only in an explicit perf-only build feature and are rejected by release-default configuration.

**Step 3: Run the full verification suite before hardware acceptance**

Run:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
npm.cmd --prefix apps/rshare-desktop-frontend ci
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
git diff --check
git status --short
```

Expected: all commands PASS; status contains only intentional tracked work.

**Step 4: Run dual-machine control acceptance**

On both machines run preflight, arm the same run id/roles, execute the physical 1000 Hz mouse scenario for 660 seconds, then merge with the control budget. Produce one exactly-five-paired-run batch per routing direction; preserve all raw artifacts and never replace an individual failed/outlying run:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/perf/windows/Invoke-RSharePerfPreflight.ps1
powershell -ExecutionPolicy Bypass -File scripts/perf/windows/Start-RSharePerfCapture.ps1 -RunId <UUID> -Role <source-or-target> -Scenario Control -DurationSeconds 660 -Output C:\tmp\rshare-perf\<role>.json
powershell -ExecutionPolicy Bypass -File scripts/perf/windows/Merge-RSharePerfRun.ps1 -Source C:\tmp\rshare-perf\source.json -Target C:\tmp\rshare-perf\target.json -Budget perf\budgets\windows-dual.toml -Output C:\tmp\rshare-perf\combined-control.json
```

Expected:

- externally/common-clock measured mouse capture→inject p50 ≤3 ms, p95 ≤6 ms, p99 ≤10 ms;
- externally/common-clock measured reliable key/button p95 ≤8 ms, p99 ≤15 ms;
- externally/common-clock measured jitter p99-p50 ≤4 ms;
- zero reliable loss/duplication and stuck modifiers over ten minutes;
- a 100 ms stall converges within 20 ms without stale pointer replay;
- bulk/media adds ≤5 ms to control p99;
- slow peer adds ≤2 ms to fast peer p99.

The checklist requires two Windows 11 machines on the same gigabit switch, Wi-Fi disabled, identical release commit/toolchain/build fingerprints, recorded scenario parameters/seed and NIC/CPU/GPU/driver/display/DPI/virtual-desktop geometry, both routing directions and quick return, mixed key/button/wheel load for ten minutes, negative-coordinate/mixed-resolution displays, WPR/ETW traces, cross-clock/external-rig calibration evidence, RSS, queue high-water marks, and a final physical check for stuck modifiers/buttons. Software local-stage and RTT-estimate reports remain useful diagnostics but are labeled `estimate` and excluded from the absolute verdict.

**Step 5: Run physical-display and IDD media acceptance**

Arm both machines again with scenario `Media`, exercise physical then IDD display, copy artifacts, and merge:

```powershell
powershell -ExecutionPolicy Bypass -File scripts/perf/windows/Start-RSharePerfCapture.ps1 -RunId <UUID> -Role <source-or-target> -Scenario Media -DurationSeconds 1800 -DisplayId <SELECTED_DISPLAY_ID> -Output C:\tmp\rshare-perf\<role>.json
powershell -ExecutionPolicy Bypass -File scripts/perf/windows/Merge-RSharePerfRun.ps1 -Source C:\tmp\rshare-perf\source.json -Target C:\tmp\rshare-perf\target.json -Budget perf\budgets\windows-media.toml -Output C:\tmp\rshare-perf\combined-media.json
```

Expected:

- 1920x1080 at 60 fps;
- glass-to-glass p95 ≤65 ms and p99 ≤90 ms;
- input overlay remains at control-path latency;
- no expired incomplete frame is presented;
- 30-minute media + 1000 Hz input has bounded RSS/queues and no control residue.

Measure true glass-to-glass with a high-speed camera or external light/input rig. Software timestamps alone are labeled capture-to-present proxy, not glass-to-glass.

**Step 6: Record evidence and release decision**

Populate `docs/performance/2026-07-28-dual-machine-results.md` with:

- commit and build profile;
- both machine/hardware/driver/network inventories;
- exact commands;
- raw artifact paths/checksums;
- scenario/config/seed, binary/lockfile hashes, build profile/features/RUSTFLAGS, and runner fingerprints;
- measurement method, bounded uncertainty, and calibration/external-rig evidence for every cross-machine absolute metric;
- all p50/p95/p99/max values;
- failures, retries, and deviations;
- separate PASS/PENDING/FAIL for control, UI, physical display, and IDD;
- go/no-go decision.

Update roadmap only for gates actually passed. If no second machine, signed driver, compatible encoder/decoder, camera/external timing rig, or ≤0.5 ms documented clock-correlation evidence is available, mark the corresponding gate `PENDING`; do not invent numbers, promote RTT estimates to measurements, or mark the architecture complete.

**Step 7: Commit**

```powershell
git add tools/rshare-perf/src/dual.rs scripts/perf/windows/Invoke-RSharePerfPreflight.ps1 scripts/perf/windows/Start-RSharePerfCapture.ps1 scripts/perf/windows/Merge-RSharePerfRun.ps1 scripts/perf/windows/rshare-control.wprp docs/performance/2026-07-28-dual-machine-results.md docs/performance/phase1-windows-validation.md docs/performance/media-windows-validation.md docs/roadmap.md
git commit -m "docs: record low latency dual machine acceptance"
```

## Final Verification

After Task 30 and before opening a pull request:

```powershell
$env:CARGO_TARGET_DIR = 'C:\tmp\rshare-low-latency-target'
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --locked
cargo bench -p rshare-core --bench control_hot_path --no-run --locked
cargo bench -p rshare-net --bench quic_loopback --no-run --locked
cargo bench -p rshare-media --bench media_pipeline --no-run --locked
npm.cmd --prefix apps/rshare-desktop-frontend ci
npm.cmd --prefix apps/rshare-desktop-frontend test
npm.cmd --prefix apps/rshare-desktop-frontend run build
git diff --check
git status --short
```

Expected: every command exits 0, performance artifacts satisfy the documented fixed-runner gates, and the worktree contains no unrelated/untracked build output.

## Rollback Boundaries

- Tasks 1-2 are measurement-only and may merge independently.
- Tasks 3-13 form the protocol-v3/control migration and must ship together; protocol v3 rejects older nodes.
- Tasks 14-20 form the local IPC/UI migration and may ship after Phase 1B.
- Tasks 21-24 are media contracts/simulation and may ship behind a disabled capability.
- Tasks 25-28 remain behind a Windows media feature/capability gate until hardware acceptance.
- Any media regression can disable media capability without rolling back the v3 low-latency control path.
