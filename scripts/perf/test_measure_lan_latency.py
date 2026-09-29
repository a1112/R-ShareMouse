import importlib.util
from pathlib import Path
import socket
import struct
import time
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "lan_latency", Path(__file__).with_name("measure-lan-latency.py")
)
latency = importlib.util.module_from_spec(spec)
spec.loader.exec_module(latency)


class LanLatencyTests(unittest.TestCase):
    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix daemon transport")
    def test_connect_uses_same_user_daemon_socket(self):
        with patch.object(latency, "daemon_socket_path", return_value=Path("/tmp/daemon.sock")), \
                patch.object(socket, "socket") as make_socket:
            deadline = time.monotonic() + 1
            stream = latency.connect(deadline)
            make_socket.assert_called_once_with(socket.AF_UNIX, socket.SOCK_STREAM)
            stream.connect.assert_called_once_with("/tmp/daemon.sock")
            self.assertGreater(stream.settimeout.call_args.args[0], 0)

    @unittest.skipUnless(hasattr(socket, "AF_UNIX"), "Unix daemon transport")
    def test_failed_connect_closes_socket(self):
        with patch.object(latency, "daemon_socket_path", return_value=Path("/tmp/daemon.sock")), \
                patch.object(socket, "socket") as make_socket:
            make_socket.return_value.connect.side_effect = ConnectionRefusedError()
            with self.assertRaises(ConnectionRefusedError):
                latency.connect(time.monotonic() + 1)
            make_socket.return_value.close.assert_called_once()

    def test_failed_samples_cannot_be_hidden_by_fast_successes(self):
        summary = latency.summarize([{"rtt_us": 1}, {"error": "timeout"}], 2, 5000)
        self.assertFalse(summary["pass"])
        self.assertEqual(summary["failed"], 1)
        self.assertFalse(latency.summarize([], 100, 5000)["pass"])

    def test_strict_budget_and_tail_percentile(self):
        rows = [{"rtt_us": 4000}] * 94 + [{"rtt_us": 5000}] * 6
        self.assertFalse(latency.summarize(rows, 100, 5000)["pass"])
        self.assertTrue(latency.summarize([{"rtt_us": 4999}], 1, 5000)["pass"])

    def test_ack_must_match_peer_sequence_and_direction(self):
        event = {
            "event_kind": "latency_probe_ack", "device_id": "peer",
            "payload": {"probe_sequence": "42", "raw_round_trip_us": "4999"},
        }
        self.assertEqual(latency.matched_rtt(event, "peer", 42), 4999)
        self.assertIsNone(latency.matched_rtt(event, "other", 42))
        self.assertIsNone(latency.matched_rtt(event, "peer", 43))
        event["event_kind"] = "latency_endpoint_switch_ack"
        self.assertIsNone(latency.matched_rtt(event, "peer", 42))

    def test_oversized_ipc_frame_is_rejected_before_body_read(self):
        reader, writer = socket.socketpair()
        with reader, writer:
            writer.sendall(struct.pack(">IB", 4 * 1024 * 1024 + 1, 1))
            with self.assertRaises(ValueError):
                latency.receive(reader, time.monotonic() + 1)


if __name__ == "__main__":
    unittest.main()
