# Low-Latency Feedback Manual Validation

Date: 2026-05-22

## Goal

Validate that operators can see daemon-owned feedback for local input, remote latency probes, and transport health before latency optimization work starts.

## Preconditions

- Two machines are on the same LAN and run compatible R-ShareMouse builds.
- The daemon is running on the local machine.
- At least one remote device is discovered and connected.
- Desktop UI is open on the Devices page.

## CLI Checks

Run:

```bash
cargo run -p rshare-cli -- status --detailed
```

Expected:

- The `Latency Feedback` section is present.
- Local input shows `idle`, `healthy`, `degraded`, or `unavailable`.
- Transport shows QUIC health, datagram availability, and RTT when connected.
- Each connected remote device shows pending, healthy/degraded RTT metrics, timeout, or unavailable state without requiring log inspection.

## Desktop Checks

1. Open the desktop app and select a remote device on the Devices page.
2. Click `网络延时探测`.
3. Confirm the latency panel immediately shows a pending state.
4. Wait for the result.

Expected:

- Successful ACKs show RTT, estimated one-way latency, raw RTT, and remote processing time.
- High RTT or degraded daemon feedback renders as a warning state.
- Missing ACKs render as a failure state with timeout text.
- If the daemon status includes `latency_feedback`, the desktop summary follows that daemon feedback instead of only inferring from recent events.

## Follow-Up Before Optimization

Record the baseline local input status, transport RTT, and remote latency metrics from the same network conditions that will be used for optimization comparisons.
