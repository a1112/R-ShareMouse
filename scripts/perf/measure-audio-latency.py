#!/usr/bin/env python3
"""Measure a known pulse train recorded on two channels of ONE PCM WAV.

This measures the difference between two physical observations on a common ADC
clock, including the audio paths between those observations. It is not RTT/2.
Use identical probe/reference routing for comparisons; calibrate ADC/DAC channel
skew separately. Emit >=100 isolated impulses at >=100 ms spacing. Record the
reference and received paths simultaneously. Float WAV is deliberately rejected;
export the reference recording as 16/24/32-bit integer PCM without resampling.
"""
import argparse
import hashlib
import json
from pathlib import Path
import wave


def percentile(values, fraction):
    values = sorted(values)
    index = (len(values) - 1) * fraction
    low = int(index)
    high = min(low + 1, len(values) - 1)
    return values[low] + (values[high] - values[low]) * (index - low)


def measure(path, reference=0, received=1, threshold=0.1, minimum_pulses=100,
            max_latency_ms=10.0, search_ms=80.0):
    if reference == received or not 0 < threshold < 1 or minimum_pulses < 1:
        raise ValueError("invalid channels, threshold, or minimum pulse count")
    if not 0 < max_latency_ms <= search_ms < 100:
        raise ValueError("require 0 < latency limit <= search window < 100 ms")
    edges = [[], []]
    with wave.open(str(path), "rb") as wav:
        rate, channels, width = wav.getframerate(), wav.getnchannels(), wav.getsampwidth()
        if rate not in (48000, 96000) or width not in (2, 3, 4):
            raise ValueError("reference WAV must be 48/96 kHz integer PCM16/24/32")
        if min(reference, received) < 0 or max(reference, received) >= channels:
            raise ValueError("reference channel is not present in recording")
        spacing = rate // 10
        last = [-spacing, -spacing]
        position = 0
        while data := wav.readframes(8192):
            frame_bytes = channels * width
            if len(data) % frame_bytes:
                raise ValueError("truncated PCM frame")
            for offset in range(0, len(data), frame_bytes):
                for side, channel in enumerate((reference, received)):
                    start = offset + channel * width
                    amplitude = abs(int.from_bytes(data[start:start + width], "little", signed=True)) / (1 << (width * 8 - 1))
                    if amplitude >= threshold and position - last[side] >= spacing:
                        edges[side].append(position)
                        last[side] = position
                position += 1
    latency, missing, unmatched, cursor = [], 0, 0, 0
    for origin in edges[0]:
        while cursor < len(edges[1]) and edges[1][cursor] < origin:
            unmatched += 1
            cursor += 1
        if cursor < len(edges[1]) and (edges[1][cursor] - origin) * 1000 / rate <= search_ms:
            latency.append((edges[1][cursor] - origin) * 1000 / rate)
            cursor += 1
        else:
            missing += 1
    unmatched += len(edges[1]) - cursor
    p95 = percentile(latency, 0.95) if latency else None
    with Path(path).open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    return {
        "method": "common_adc_clock_pulse_train",
        "recording_sha256": digest,
        "sample_rate": rate,
        "duration_seconds": position / rate,
        "reference_channel": reference,
        "received_channel": received,
        "reference_pulses": len(edges[0]),
        "matched_pulses": len(latency),
        "missing_pulses": missing,
        "unmatched_received_pulses": unmatched,
        "one_way_p50_ms": percentile(latency, 0.5) if latency else None,
        "one_way_p95_ms": p95,
        "one_way_p99_ms": percentile(latency, 0.99) if latency else None,
        "limit_ms": max_latency_ms,
        "passed": len(latency) >= minimum_pulses and missing == 0 and unmatched == 0 and p95 is not None and p95 <= max_latency_ms,
        "scope": "Measured signal path only; does not establish eight-channel load, underruns, keyboard latency, or soak acceptance.",
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("recording", type=Path)
    parser.add_argument("--reference-channel", type=int, default=0)
    parser.add_argument("--received-channel", type=int, default=1)
    parser.add_argument("--threshold", type=float, default=0.1)
    parser.add_argument("--minimum-pulses", type=int, default=100)
    parser.add_argument("--max-latency-ms", type=float, default=10)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = measure(args.recording, args.reference_channel, args.received_channel,
                     args.threshold, args.minimum_pulses, args.max_latency_ms)
    text = json.dumps(result, indent=2, ensure_ascii=False)
    if args.output:
        args.output.write_text(text + "\n")
    print(text)
    return 0 if result["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
