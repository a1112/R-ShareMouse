#!/usr/bin/env python3
"""Native iOS H.264 receiver test using simulator loopback, NOT physical USB.

Requires a dedicated booted simulator, ffmpeg and Xcode. Generated video only.
The production USB listener, framing, decoder and presentation remain unchanged.
"""
import json
import os
from pathlib import Path
import re
import secrets
import socket
import struct
import ssl
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "target/mac-usb-display"
OUT.mkdir(parents=True, exist_ok=True)
SIMULATOR = os.environ["RSHARE_IOS_SIMULATOR"]
lan = os.environ.get("RSHARE_DISPLAY_TEST_TRANSPORT") == "lan"
code = secrets.token_hex(32) if lan else f"{secrets.randbelow(1_000_000):06d}"
port = 27439 if lan else 27438
native_fixture = os.environ.get("RSHARE_NATIVE_ENCODER_FIXTURE")
source = OUT / "receiver-fixture.h264"
if not native_fixture:
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i",
    "testsrc2=size=1280x720:rate=30", "-t", "3", "-c:v", "h264_videotoolbox", "-b:v", "4M", "-bf", "0",
    "-g", "30", "-bsf:v", "h264_metadata=aud=insert", "-f", "h264", str(source)], check=True)
nals = [] if native_fixture else [item for item in re.split(b"\x00\x00(?:\x00)?\x01", source.read_bytes()) if item]
frames, group = [], []
for nal in nals:
    if nal[0] & 31 == 9:
        if group:
            frames.append(group)
        group = []
    else:
        group.append(nal)
if group:
    frames.append(group)
packets = []
for group in frames:
    sps = next((n for n in group if n[0] & 31 == 7), b"")
    pps = next((n for n in group if n[0] & 31 == 8), b"")
    video = b"".join(struct.pack(">I", len(n)) + n for n in group if n[0] & 31 not in (7, 8))
    if any(n[0] & 31 in (1, 5) for n in group):
        packets.append(struct.pack(">HH", len(sps), len(pps)) + sps + pps + video)
if native_fixture:
    packets = [Path(native_fixture).read_bytes()]
assert packets
result = {"transport": "simulator TLS-PSK loopback (NOT physical LAN)" if lan else "simulator-loopback (NOT USB)",
    "source": "RShare native VideoToolbox encoder, generated green pixels" if native_fixture else "ffmpeg generated H264",
    "frames_confirmed": 0}
stopping = threading.Event()

def read_exact(conn, size):
    data = b""
    while len(data) < size:
        chunk = conn.recv(size - len(data))
        if not chunk:
            raise EOFError("Receiver closed")
        data += chunk
    return data

def exchange(conn, kind, data):
    conn.sendall(struct.pack(">IB", len(data), kind) + data)
    size, kind = struct.unpack(">IB", read_exact(conn, 5))
    assert kind == 3 and size <= 4096
    return json.loads(read_exact(conn, size))

def sender():
    try:
        deadline = time.monotonic() + 120
        while not stopping.is_set():
            try:
                conn = socket.create_connection(("127.0.0.1", port), timeout=3)
                break
            except OSError:
                if time.monotonic() > deadline:
                    raise TimeoutError("Simulator USB receiver did not listen")
                time.sleep(.2)
        else:
            return
        def secure(conn, key):
            if not lan:
                return conn
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE  # TLS PSK authenticates both peers; no certificate path.
            context.minimum_version = context.maximum_version = ssl.TLSVersion.TLSv1_2
            context.set_ciphers("PSK-AES128-GCM-SHA256")
            context.set_psk_client_callback(lambda hint: ("rshare-display-v1", bytes.fromhex(key)))
            return context.wrap_socket(conn)
        if lan:
            try:
                secure(conn, secrets.token_hex(32))
                raise AssertionError("Wrong TLS key must fail")
            except ssl.SSLError:
                result["wrong_tls_key_rejected"] = True
            conn = socket.create_connection(("127.0.0.1", port), timeout=3)
        # Reject unauthenticated video and a wrong code before decoder creation.
        for kind, body in [(2, b""), (1, json.dumps({"version":1,"code":"not-the-code"}).encode())]:
            conn = secure(conn, code)
            with conn:
                conn.settimeout(3)
                conn.sendall(struct.pack(">IB", len(body), kind) + body)
                assert conn.recv(1) == b"", "Unauthenticated client must be disconnected"
            conn = socket.create_connection(("127.0.0.1", port), timeout=3)
        result["unauthenticated_clients_rejected"] = 2
        with secure(conn, code) as conn:
            conn.settimeout(5)
            assert exchange(conn, 1, json.dumps({"version": 1, "code": code}).encode())["accepted"]
            sequence = 0
            while not stopping.is_set():
                for packet in packets:
                    if stopping.is_set():
                        return
                    sequence += 1
                    body = struct.pack(">Q", sequence) + packet
                    ack = exchange(conn, 2, body)
                    assert ack["sequence"] == sequence and ack["frames_decoded"] <= sequence
                    result["frames_confirmed"] = ack["frames_decoded"]
                    time.sleep(1 / 30)
    except (EOFError, OSError) as error:
        result["connection_end"] = str(error)
    except Exception as error:
        result["error"] = repr(error)

worker = threading.Thread(target=sender, daemon=True)
worker.start()
bundle = OUT / f"native-usb-receiver-{int(time.time())}.xcresult"
try:
    test = subprocess.run(["xcodebuild", "-project", "apps/rshare-ios/RShareMouse.xcodeproj", "-scheme", "RShareMouse",
        "-destination", f"platform=iOS Simulator,id={SIMULATOR}", "-derivedDataPath", "target/mac-usb-display/ios-tests",
        "-resultBundlePath", str(bundle),
        "-only-testing:RShareMouseUITests/LiveClientUITests/testNativeUSBVideoDecoderFullscreenAndBackgroundCleanup", "test"],
        cwd=ROOT, env={**os.environ, "RSHARE_USB_TEST_CODE": code,
            "RSHARE_DISPLAY_TEST_TRANSPORT": "lan" if lan else "usb"}, timeout=180)
    result["test_exit_code"] = test.returncode
    assert test.returncode == 0 and result["frames_confirmed"] > 0 and "error" not in result, result
finally:
    stopping.set(); worker.join(timeout=6)
    result["result_bundle"] = str(bundle)
    (OUT / ("native-lan-receiver.json" if lan else "native-usb-receiver.json")).write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
