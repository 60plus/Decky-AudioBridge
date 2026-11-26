import json
import os
import sys
import tempfile
import threading
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

from fake_receiver import FakeReceiver  # noqa: E402

from audiobridge import engine, vban  # noqa: E402


class EngineTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.status = os.path.join(self.tmp.name, "run", "status.json")

    def tearDown(self):
        self.tmp.cleanup()

    def run_engine(self, eng, seconds):
        thread = threading.Thread(target=eng.run)
        # signal handlers can only be set from the main thread; run() sets them,
        # so call the loop body directly in a thread with signals stubbed out.
        original = engine.signal.signal
        engine.signal.signal = lambda *a: None
        try:
            thread.start()
            time.sleep(seconds)
            eng.request_stop()
            thread.join(5)
        finally:
            engine.signal.signal = original
        self.assertFalse(thread.is_alive())

    def read_status(self):
        with open(self.status) as handle:
            return json.load(handle)

    def test_streams_tone_and_sees_receiver(self):
        with FakeReceiver() as rx:
            eng = engine.Engine("127.0.0.1", rx.port, "balanced", "TestStream", self.status, source="tone")
            states = []
            original = eng.write_status

            def spy(now):
                original(now)
                states.append(self.read_status()["state"])
            eng.write_status = spy
            self.run_engine(eng, 1.6)
        self.assertIn("streaming", states)
        self.assertGreater(len(rx.audio), 400)       # about 375 packets per second
        header, payload = rx.audio[0]
        self.assertEqual(header.rate, 48000)
        self.assertEqual(header.nbs + 1, 128)
        self.assertEqual(header.nbc + 1, 2)
        self.assertEqual(header.bits, vban.DATATYPE_INT16)
        self.assertEqual(header.stream_name, "TestStream")
        self.assertEqual(len(payload), 128 * 2 * 2)
        frames = [h.frame for h, _ in rx.audio]
        self.assertEqual(frames, list(range(frames[0], frames[0] + len(frames))))
        self.assertTrue(rx.pings)
        comment = vban.parse_comment(rx.pings[0][1].comment)
        self.assertEqual(comment.get("preset"), "balanced")
        final = self.read_status()
        self.assertEqual(final["state"], "stopped")
        self.assertEqual(final["receiver"]["port"], rx.port)

    def test_high_preset_sends_24_bit(self):
        with FakeReceiver() as rx:
            eng = engine.Engine("127.0.0.1", rx.port, "high", "S", self.status, source="tone")
            self.run_engine(eng, 0.5)
        header, payload = rx.audio[0]
        self.assertEqual(header.bits, vban.DATATYPE_INT24)
        self.assertEqual(len(payload), (header.nbs + 1) * 2 * 3)

    def test_silent_receiver_reports_no_reply(self):
        with FakeReceiver(answer=False) as rx:
            eng = engine.Engine("127.0.0.1", rx.port, "low", "S", self.status, source="tone")
            last = {}
            original = eng.write_status

            def spy(now):
                original(now)
                last.update(self.read_status())
            eng.write_status = spy
            self.run_engine(eng, engine.REPLY_TIMEOUT + 0.8)
        self.assertEqual(last["state"], "no_reply")
        self.assertFalse(last["receiver"]["answering"])
        self.assertTrue(rx.audio)
        self.assertEqual(rx.audio[0][0].nbs + 1, 64)

    def test_test_sound_replaces_audio(self):
        with FakeReceiver() as rx:
            eng = engine.Engine("127.0.0.1", rx.port, "balanced", "S", self.status, source="tone")
            eng.request_chime()
            self.run_engine(eng, 0.4)
        chime = engine.make_chime(vban.DATATYPE_INT16)
        self.assertEqual(rx.audio[0][1], chime[:len(rx.audio[0][1])])

    def test_unresolvable_host(self):
        eng = engine.Engine("no-such-host.invalid", 6980, "balanced", "S", self.status, source="tone")
        self.run_engine(eng, 1.0)
        self.assertIsNone(eng.target)
        self.assertEqual(eng.state(time.monotonic())[0], "stopping")
        eng.stopping = False
        self.assertIn(eng.state(time.monotonic())[0], ("receiver_not_found", "connecting"))

    def test_capture_command_prefers_pw_record(self):
        original_find, original_raw = engine.find_tool, engine.pw_record_supports_raw
        try:
            engine.find_tool = lambda name: "/usr/bin/" + name
            engine.pw_record_supports_raw = lambda path: True
            cmd, tool = engine.capture_command(vban.DATATYPE_INT16, 512)
            self.assertEqual(tool, "pw-record")
            self.assertIn("stream.capture.sink = true", cmd[cmd.index("-P") + 1])
            self.assertEqual(cmd[-1], "-")
            engine.pw_record_supports_raw = lambda path: False
            cmd, tool = engine.capture_command(vban.DATATYPE_INT24, 512)
            self.assertEqual(tool, "parec")
            self.assertIn("--device=@DEFAULT_MONITOR@", cmd)
            self.assertIn("--format=s24le", cmd)
            engine.find_tool = lambda name: None
            self.assertEqual(engine.capture_command(vban.DATATYPE_INT16, 512), (None, ""))
        finally:
            engine.find_tool, engine.pw_record_supports_raw = original_find, original_raw

    def test_peak(self):
        self.assertEqual(engine.peak_of(b"", vban.DATATYPE_INT16), 0.0)
        loud = engine.encode_samples([0.5, -1.0], vban.DATATYPE_INT16)
        self.assertAlmostEqual(engine.peak_of(loud, vban.DATATYPE_INT16), 1.0, places=3)
        quiet = engine.encode_samples([0.25, 0.0], vban.DATATYPE_INT24)
        self.assertAlmostEqual(engine.peak_of(quiet, vban.DATATYPE_INT24), 0.25, places=1)


if __name__ == "__main__":
    unittest.main()
