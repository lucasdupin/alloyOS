#!/usr/bin/env python3
"""Exercise the native JSON-lines reader without devices, credentials, or network.

Run after scripts/audio/build.sh. Keep stdin open while awaiting every reply:
closing it would conceal the short-read deadlock this test guards against.
"""
import json
from pathlib import Path
import select
import subprocess
import unittest

HELPER = Path(__file__).resolve().parents[2] / "target/alloyos-voice-helper"


class HelperPipeTests(unittest.TestCase):
    def setUp(self):
        self.helper = subprocess.Popen(
            [str(HELPER)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        self.pending = bytearray()

    def tearDown(self):
        self.helper.kill()
        self.helper.wait(timeout=3)
        self.helper.stdin.close()
        self.helper.stdout.close()

    def send(self, data):
        self.helper.stdin.write(data)
        self.helper.stdin.flush()

    def event(self):
        import os
        import time
        deadline = time.monotonic() + 3
        while b"\n" not in self.pending:
            self.assertTrue(
                select.select([self.helper.stdout], [], [], max(0, deadline - time.monotonic()))[0],
                "helper did not reply to a short command while stdin stayed open",
            )
            chunk = os.read(self.helper.stdout.fileno(), 4096)
            self.assertTrue(chunk, "helper exited before replying")
            self.pending.extend(chunk)
        line, _, remaining = self.pending.partition(b"\n")
        self.pending = bytearray(remaining)
        return json.loads(line)

    def test_device_free_capture_conversion(self):
        result = subprocess.run([str(HELPER), "--test-converter"],
                                capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))
        self.assertIn(b"PCM16 mono16k passed", result.stdout)

    def test_short_stop_with_open_input(self):
        self.send(b'{"op":"stop","session":0}\n')
        self.assertEqual(self.event(), {"event": "stopped", "session": 0})
        self.assertIsNone(self.helper.poll())

    def test_fragmented_and_multiple_commands(self):
        self.send(b'{"op":"sto')
        self.assertFalse(select.select([self.helper.stdout], [], [], .05)[0])
        self.send(b'p","session":0}\n{"op":"stop","session":0}\n')
        for _ in range(2):
            self.assertEqual(self.event(), {"event": "stopped", "session": 0})

    def test_legacy_transport_rejected_before_permission_or_network(self):
        self.send(json.dumps({"op": "start", "session": 8,
            "endpoint": "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent",
            "api_key": "fixture-no-network"}).encode() + b"\n")
        self.assertEqual(self.event(), {"event": "error", "session": 8,
            "message": "Voice transport mismatch; rebuild kernel and restart QEMU"})
        self.assertFalse(select.select([self.helper.stdout], [], [], .05)[0])

    def test_start_validation_replies_without_opening_devices(self):
        self.send(b'{"op":"start","session":7,"endpoint":"invalid","api_key":""}\n')
        self.assertEqual(self.event(), {
            "event": "error", "session": 7, "message": "invalid voice endpoint",
        })
        self.send(b'{"op":"stop","session":7}\n')
        self.assertEqual(self.event(), {"event": "stopped", "session": 7})


if __name__ == "__main__":
    unittest.main()
