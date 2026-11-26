import os
import sys
import tempfile
import time
import unittest

sys.path.insert(0, os.path.dirname(__file__))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

from fake_receiver import FakeReceiver  # noqa: E402

from audiobridge import diagnostics, discovery  # noqa: E402
from audiobridge.backend import Backend, UserError, describe  # noqa: E402
from audiobridge.service import EngineControl, EngineError  # noqa: E402


class FakeControl:
    def __init__(self):
        self.is_running = False
        self.status = None
        self.started = []
        self.stopped = 0
        self.problem_list = []
        self.chimes = 0
        self.session = True

    def session_ready(self):
        return self.session

    def running(self):
        return self.is_running, (123 if self.is_running else None)

    def read_status(self):
        return self.status

    def problems(self):
        return list(self.problem_list)

    def start(self, settings):
        if not settings.get("receiver_host"):
            raise EngineError("Choose a receiver first.")
        self.started.append(dict(settings))
        self.is_running = True

    def stop(self):
        self.stopped += 1
        self.is_running = False

    def test_sound(self):
        if not self.is_running:
            return False
        self.chimes += 1
        return True

    def journal(self, lines=40):
        return "INFO engine started: 192.168.0.40:6980"


class BackendTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.control = FakeControl()
        self.backend = Backend(self.tmp.name, "/nonexistent", control=self.control)

    def tearDown(self):
        self.tmp.cleanup()

    def test_start_needs_receiver(self):
        with self.assertRaises(UserError):
            self.backend.start()
        state = self.backend.state()
        self.assertEqual(state["view"]["title"], "Not streaming")

    def test_set_address_then_start_and_stop(self):
        state = self.backend.set_address("192.168.0.40:7000")
        self.assertEqual(state["settings"]["receiver_host"], "192.168.0.40")
        self.assertEqual(state["settings"]["receiver_port"], 7000)
        self.backend.start()
        self.assertEqual(self.control.started[-1]["receiver_port"], 7000)
        self.backend.stop()
        self.assertFalse(self.control.is_running)

    def test_bad_address_is_user_error(self):
        with self.assertRaises(UserError):
            self.backend.set_address("not an address")
        with self.assertRaises(UserError):
            self.backend.set_address("")

    def test_changing_receiver_restarts_running_engine(self):
        self.backend.choose_receiver("192.168.0.40", 6980, "WORK-PC")
        self.backend.start()
        self.backend.update_settings({"auto_start": True})
        self.assertEqual(len(self.control.started), 1)
        self.backend.update_settings({"preset": "low"})
        self.assertEqual(len(self.control.started), 2)
        self.assertEqual(self.control.started[-1]["preset"], "low")

    def test_autostart(self):
        self.assertIs(self.backend.autostart(), False)
        self.backend.update_settings({"receiver_host": "192.168.0.40", "auto_start": True})
        self.control.session = False
        self.assertIsNone(self.backend.autostart())   # session not up yet: try again later
        self.assertFalse(self.control.started)
        self.control.session = True
        self.assertIs(self.backend.autostart(), True)
        self.assertIs(self.backend.autostart(), False)   # already running, left alone

    def test_test_sound(self):
        with self.assertRaises(UserError):
            self.backend.test_sound()
        self.backend.set_address("192.168.0.40")
        self.backend.start()
        self.assertTrue(self.backend.test_sound())

    def test_diagnostics_redacts_addresses(self):
        self.backend.set_address("192.168.0.40")
        text = self.backend.diagnostics(redact=True)
        self.assertNotIn("192.168.0.40", text)
        self.assertIn("192.168.x.x", text)
        self.assertIn("192.168.0.40", self.backend.diagnostics(redact=False))


class DescribeTests(unittest.TestCase):
    settings = {"receiver_host": "192.168.0.40", "receiver_port": 6980, "receiver_name": ""}

    def status(self, state, **extra):
        data = {"state": state, "detail": "x", "stale": False,
                "receiver": {"ip": "192.168.0.40", "name": "WORK-PC", "rtt_ms": 1.4}}
        data.update(extra)
        return data

    def test_states(self):
        self.assertEqual(describe(self.settings, True, self.status("streaming"), [])["tone"], "ok")
        self.assertIn("WORK-PC", describe(self.settings, True, self.status("streaming"), [])["detail"])
        self.assertEqual(describe(self.settings, True, self.status("no_reply"), [])["tone"], "warn")
        self.assertEqual(describe(self.settings, True, self.status("receiver_not_found"), [])["tone"], "error")
        self.assertEqual(describe(self.settings, True, self.status("capture_lost"), [])["tone"], "warn")
        self.assertEqual(describe(self.settings, True, None, [])["tone"], "busy")
        stale = self.status("streaming", stale=True)
        self.assertEqual(describe(self.settings, True, stale, [])["tone"], "warn")
        crashed = self.status("streaming")
        self.assertEqual(describe(self.settings, False, crashed, [])["title"], "Streaming stopped unexpectedly")
        self.assertEqual(describe(self.settings, False, None, ["No tool"])["tone"], "error")
        self.assertEqual(describe({"receiver_host": ""}, False, None, [])["tone"], "idle")


class DiscoveryTests(unittest.TestCase):
    def test_finds_receiver(self):
        with FakeReceiver(name="WORK-PC") as rx:
            found = discovery.discover(port=rx.port, timeout=0.8, addresses=["127.0.0.1"])
        self.assertEqual(len(found), 1)
        self.assertEqual(found[0]["name"], "WORK-PC")
        self.assertEqual(found[0]["ip"], "127.0.0.1")
        self.assertTrue(found[0]["ours"])

    def test_nothing_answers(self):
        with FakeReceiver(answer=False) as rx:
            self.assertEqual(discovery.discover(port=rx.port, timeout=0.5, addresses=["127.0.0.1"]), [])


class ServiceTests(unittest.TestCase):
    def test_command_line(self):
        control = EngineControl("/plugin/py_modules", runtime_dir="/run/user/1000", python="/usr/bin/python3")
        cmd = control.command({"receiver_host": "192.168.0.40", "receiver_port": 6980,
                               "preset": "balanced", "stream_name": "AudioBridge"})
        self.assertEqual(cmd[:4], ["/usr/bin/python3", "-s", "-E", "/plugin/py_modules/audiobridge_sender.py"])
        self.assertIn("--status-file", cmd)
        self.assertEqual(cmd[cmd.index("--status-file") + 1], "/run/user/1000/audiobridge/status.json")
        self.assertEqual(control.env()["XDG_RUNTIME_DIR"], "/run/user/1000")

    def test_session_ready(self):
        with tempfile.TemporaryDirectory() as tmp:
            control = EngineControl("/x", runtime_dir=tmp, python="/usr/bin/python3")
            self.assertFalse(control.session_ready())
            open(os.path.join(tmp, "bus"), "w").close()
            self.assertTrue(control.session_ready())

    def test_status_staleness(self):
        with tempfile.TemporaryDirectory() as tmp:
            control = EngineControl("/x", runtime_dir=tmp, python="/usr/bin/python3")
            self.assertIsNone(control.read_status())
            from audiobridge.engine import write_json_atomic
            write_json_atomic(control.status_file, {"state": "streaming", "updated_at": time.time()})
            self.assertFalse(control.read_status()["stale"])
            write_json_atomic(control.status_file, {"state": "streaming", "updated_at": time.time() - 60})
            self.assertTrue(control.read_status()["stale"])

    def test_redact(self):
        self.assertEqual(diagnostics.redact_text("to 192.168.0.40:6980"), "to 192.168.x.x:6980")


if __name__ == "__main__":
    unittest.main()
