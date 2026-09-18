import importlib.util
from pathlib import Path
import tempfile
import unittest
import wave

spec = importlib.util.spec_from_file_location("measure_audio", Path(__file__).with_name("measure-audio-latency.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class AudioLatencyTest(unittest.TestCase):
    def recording(self, path, rate=48000, delay_ms=7, missing=False):
        frames = rate * 2
        payload = bytearray(frames * 6)
        for index in range(10):
            reference = rate // 20 + index * rate // 6
            received = reference + int(rate * delay_ms / 1000)
            payload[reference*6:reference*6+3] = (4000000).to_bytes(3,"little",signed=True)
            if not (missing and index == 4):
                payload[received*6+3:received*6+6] = (4000000).to_bytes(3,"little",signed=True)
        with wave.open(str(path),"wb") as wav:
            wav.setnchannels(2); wav.setsampwidth(3); wav.setframerate(rate); wav.writeframes(payload)
    def test_common_clock_latency_and_missing_pulses(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"reference.wav"
            for rate in (48000,96000):
                self.recording(path,rate)
                result=module.measure(path,minimum_pulses=10)
                self.assertTrue(result["passed"])
                self.assertAlmostEqual(result["one_way_p95_ms"],7)
            self.recording(path,missing=True)
            result=module.measure(path,minimum_pulses=1)
            self.assertFalse(result["passed"])
            self.assertEqual(result["missing_pulses"],1)
    def test_latency_limit_and_same_channel_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path=Path(directory)/"reference.wav"
            self.recording(path,delay_ms=15)
            self.assertFalse(module.measure(path,minimum_pulses=10)["passed"])
            with self.assertRaises(ValueError): module.measure(path,reference=0,received=0)

if __name__=="__main__": unittest.main()
