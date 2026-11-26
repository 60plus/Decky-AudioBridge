import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

from audiobridge.settings import (DEFAULTS, SettingsError, SettingsStore, parse_address,  # noqa: E402
                                  validate, validate_host, validate_port)


class ValidationTests(unittest.TestCase):
    def test_hosts(self):
        self.assertEqual(validate_host(" 192.168.0.40 "), "192.168.0.40")
        self.assertEqual(validate_host("work-pc"), "work-pc")
        self.assertEqual(validate_host("work-pc.local"), "work-pc.local")
        self.assertEqual(validate_host(""), "")
        for bad in ("256.1.1.1", "1.2.3", "-pc", "pc_name", "a b", "224.0.0.1", "0.0.0.0", "::1", 5):
            with self.assertRaises(SettingsError, msg=bad):
                validate_host(bad)

    def test_ports(self):
        self.assertEqual(validate_port("6980"), 6980)
        for bad in (0, 80, 70000, "x", None, True):
            with self.assertRaises(SettingsError, msg=bad):
                validate_port(bad)

    def test_parse_address(self):
        self.assertEqual(parse_address("192.168.0.40"), ("192.168.0.40", None))
        self.assertEqual(parse_address("192.168.0.40:7000"), ("192.168.0.40", 7000))
        self.assertEqual(parse_address("work-pc:6980"), ("work-pc", 6980))
        with self.assertRaises(SettingsError):
            parse_address("192.168.0.40:80")

    def test_validate_fills_defaults_and_drops_unknown(self):
        out = validate({"preset": "low", "whatever": 1})
        self.assertEqual(out["preset"], "low")
        self.assertNotIn("whatever", out)
        self.assertEqual(out["receiver_port"], DEFAULTS["receiver_port"])
        with self.assertRaises(SettingsError):
            validate({"preset": "ultra"})
        with self.assertRaises(SettingsError):
            validate({"auto_start": "yes"})
        with self.assertRaises(SettingsError):
            validate({"stream_name": "bad;name"})


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.store = SettingsStore(os.path.join(self.tmp.name, "settings"))

    def tearDown(self):
        self.tmp.cleanup()

    def test_missing_file_gives_defaults(self):
        self.assertEqual(self.store.load(), DEFAULTS)

    def test_update_persists(self):
        self.store.update({"receiver_host": "192.168.0.40", "auto_start": True})
        again = SettingsStore(self.store.directory).load()
        self.assertEqual(again["receiver_host"], "192.168.0.40")
        self.assertTrue(again["auto_start"])
        self.assertEqual(os.listdir(self.store.directory), ["settings.json"])

    def test_update_rejects_unknown_key(self):
        with self.assertRaises(SettingsError):
            self.store.update({"evil": 1})

    def test_corrupt_file_gives_defaults(self):
        os.makedirs(self.store.directory)
        with open(self.store.path, "w") as handle:
            handle.write("{not json")
        self.assertEqual(self.store.load(), DEFAULTS)

    def test_partly_bad_file_keeps_good_values(self):
        os.makedirs(self.store.directory)
        with open(self.store.path, "w") as handle:
            json.dump({"receiver_host": "192.168.0.9", "receiver_port": 5, "preset": "high"}, handle)
        loaded = self.store.load()
        self.assertEqual(loaded["receiver_host"], "192.168.0.9")
        self.assertEqual(loaded["receiver_port"], DEFAULTS["receiver_port"])
        self.assertEqual(loaded["preset"], "high")


if __name__ == "__main__":
    unittest.main()
