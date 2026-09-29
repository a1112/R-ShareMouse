#!/usr/bin/env python3
"""Measure authenticated daemon-to-peer RTT; never injects keyboard/mouse input.

Requires a daemon exposing probe_sequence and monotonic raw_round_trip_us.
The default gate is P95 < 5 ms with zero failed samples, not a one-way estimate.
"""

import argparse
import json
import math
import socket
import statistics
import struct
import time
import uuid
from pathlib import Path


def send(sock, value):
    data = json.dumps(value).encode()
    sock.sendall(struct.pack(">IB", len(data), 1) + data)


def receive(sock, deadline):
    def read(count):
        data = bytearray()
        while len(data) < count:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("IPC response deadline exceeded")
            sock.settimeout(remaining)
            part = sock.recv(count - len(data))
            if not part:
                raise ConnectionError("daemon closed IPC")
            data.extend(part)
        return data

    size, kind = struct.unpack(">IB", read(5))
    if kind != 1 or size > 4 * 1024 * 1024:
        raise ValueError("invalid or oversized IPC JSON frame")
    value = json.loads(read(size))
    if isinstance(value, dict) and "Error" in value:
        raise RuntimeError(value["Error"])
    return value


def request(value, deadline):
    with socket.create_connection(
        ("127.0.0.1", 27435), timeout=max(0.001, deadline - time.monotonic())
    ) as sock:
        send(sock, value)
        return receive(sock, deadline)


def matched_rtt(event, peer, sequence):
    if not isinstance(event, dict):
        return None
    if event.get("event_kind") != "latency_probe_ack" or event.get("device_id") != peer:
        return None
    payload = event.get("payload", {})
    if payload.get("probe_sequence") != str(sequence):
        return None
    value = int(payload["raw_round_trip_us"])
    if value < 0:
        raise ValueError("negative RTT")
    return value


def summarize(rows, requested, budget_us):
    values = sorted(row["rtt_us"] for row in rows if "rtt_us" in row)
    failed = requested - len(values)
    percentile = lambda q: values[math.ceil(len(values) * q) - 1] if values else None
    p95 = percentile(0.95)
    return {
        "requested": requested,
        "received": len(values),
        "failed": failed,
        "min_us": min(values) if values else None,
        "mean_us": statistics.mean(values) if values else None,
        "p50_us": percentile(0.5),
        "p95_us": p95,
        "p99_us": percentile(0.99),
        "max_us": max(values) if values else None,
        "budget_us": budget_us,
        "under_budget": sum(value < budget_us for value in values),
        "pass": failed == 0 and p95 is not None and p95 < budget_us,
    }


def run(args):
    rows = []
    report = {
        "metric": "authenticated_daemon_round_trip",
        "clock": "sender_monotonic",
        "unit": "microseconds",
        "peer_id": args.device_id,
        "physical_input_verified": False,
        "samples": rows,
    }
    try:
        status = request("Status", time.monotonic() + 5)["Status"]
        report["local_status"] = status
        peers = request("Devices", time.monotonic() + 5)["Devices"]
        if not any(p["id"] == args.device_id and p["connected"] for p in peers):
            raise RuntimeError("requested peer is not connected")
        for index in range(args.samples):
            start = time.monotonic()
            deadline = start + args.timeout_ms / 1000
            row = {"index": index}
            rows.append(row)
            try:
                # A fresh subscription per sample bounds stale queued telemetry.
                with socket.create_connection(("127.0.0.1", 27435), timeout=5) as stream:
                    send(stream, "SubscribeLocalControls")
                    receive(stream, deadline)
                    result = request(
                        {"RunRemoteLatencyTest": {"device_id": args.device_id}}, deadline
                    )["LocalInputTest"]
                    if result.get("status") != "Success" or result.get("probe_sequence") is None:
                        raise RuntimeError("probe failed or daemon lacks correlated latency support")
                    sequence = result["probe_sequence"]
                    row["probe_sequence"] = sequence
                    while True:
                        event = receive(stream, deadline).get("LocalControlEvent")
                        value = matched_rtt(event, args.device_id, sequence)
                        if value is not None:
                            row["rtt_us"] = value
                            break
            except (OSError, ValueError, KeyError, RuntimeError) as error:
                row["error"] = str(error)
            time.sleep(max(0, args.interval_ms / 1000 - (time.monotonic() - start)))
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        report["error"] = str(error)
    report["summary"] = summarize(rows, args.samples, round(args.budget_ms * 1000))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    print(json.dumps(report["summary"], indent=2))
    return 0 if report["summary"]["pass"] else 1


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device-id", type=lambda value: str(uuid.UUID(value)), required=True)
    parser.add_argument("--samples", type=int, default=100)
    parser.add_argument("--interval-ms", type=int, default=100)
    parser.add_argument("--timeout-ms", type=int, default=2000)
    parser.add_argument("--budget-ms", type=float, default=5)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 1 <= args.samples <= 10000 or not 10 <= args.interval_ms <= 60000:
        parser.error("samples must be 1..10000 and interval-ms 10..60000")
    if not 100 <= args.timeout_ms <= 30000 or not 0 < args.budget_ms <= 30000:
        parser.error("timeout-ms must be 100..30000 and budget-ms 0..30000")
    raise SystemExit(run(args))
