"""main.py as Decky calls it: methods on the class, with the class as self."""

import asyncio
import logging
import os
import sys
import tempfile
import types
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


class MainTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        decky = types.ModuleType("decky")
        decky.DECKY_PLUGIN_SETTINGS_DIR = os.path.join(self.tmp.name, "settings")
        decky.logger = logging.getLogger("decky-test")
        sys.modules["decky"] = decky
        sys.path.insert(0, ROOT)
        sys.modules.pop("main", None)
        import main
        self.Plugin = main.Plugin

    def tearDown(self):
        sys.path.remove(ROOT)
        sys.modules.pop("main", None)
        self.tmp.cleanup()

    def call(self, method, *args):
        return asyncio.run(getattr(self.Plugin, method)(self.Plugin, *args))

    def test_requests(self):
        state = self.call("request", "state")
        self.assertTrue(state["ok"])
        self.assertEqual(state["data"]["settings"]["receiver_port"], 6980)
        saved = self.call("request", "set_address", {"text": "192.168.0.40"})
        self.assertTrue(saved["ok"])
        self.assertEqual(saved["data"]["settings"]["receiver_host"], "192.168.0.40")
        self.assertEqual(self.call("request", "set_address", {"text": "bad host!"})["ok"], False)
        self.assertEqual(self.call("request", "nope"), {"ok": False, "error": "Unknown request."})
        self.assertEqual(self.call("request", "state", ["not", "a", "dict"]), {"ok": False, "error": "Bad request."})
        self.assertEqual(self.call("request", "state", {"unexpected": 1}), {"ok": False, "error": "Bad request."})

    def test_unload_does_not_await(self):
        # _unload must finish in one step: the loader stops serving the loop.
        coro = self.Plugin._unload(self.Plugin)
        with self.assertRaises(StopIteration):
            coro.send(None)


if __name__ == "__main__":
    unittest.main()
