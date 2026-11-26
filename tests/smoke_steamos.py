"""Check the real engine on a SteamOS machine without Decky.

Copy the plugin folder (main.py, py_modules/) and tests/ to the machine and run
as the desktop user:

    python3 -I tests/smoke_steamos.py [--audible]

It drives main.Plugin exactly like the panel does, with a stand-in `decky`
module, against a local fake receiver. The engine runs as the real transient
user unit and captures the default output's monitor. A very quiet tone
(about -50 dBFS, or -20 dBFS with --audible) is played so the capture has
something to measure. Nothing is left behind: settings go to a temporary
folder and the unit is stopped at the end.
"""

import asyncio
import json
import math
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import types

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "py_modules"))

from fake_receiver import FakeReceiver  # noqa: E402


def stand_in_decky(settings_dir):
    import logging
    logging.basicConfig(level=logging.INFO, format="decky %(levelname)s %(message)s")
    module = types.ModuleType("decky")
    module.DECKY_PLUGIN_SETTINGS_DIR = settings_dir
    module.logger = logging.getLogger("decky")
    sys.modules["decky"] = module


def play_tone(seconds, amplitude):
    tool = shutil.which("pw-play")
    if not tool:
        return None
    frames = int(48000 * seconds)
    data = b"".join(struct.pack("<hh", v, v) for v in
                    (int(amplitude * 32767 * math.sin(2 * math.pi * 330 * i / 48000)) for i in range(frames)))
    env = dict(os.environ, XDG_RUNTIME_DIR="/run/user/%d" % os.getuid())
    proc = subprocess.Popen([tool, "--raw", "--format", "s16", "--rate", "48000", "--channels", "2",
                             "--media-role", "Notification", "-"], stdin=subprocess.PIPE, env=env)
    proc.stdin.write(data)
    proc.stdin.close()
    return proc


async def main():
    tmp = tempfile.mkdtemp(prefix="audiobridge-smoke-")
    stand_in_decky(os.path.join(tmp, "settings"))
    sys.path.insert(0, ROOT)
    import main as plugin_main

    plugin = plugin_main.Plugin()
    await plugin._main()
    try:
        await run_checks(plugin)
    finally:
        await plugin.request("stop")
        shutil.rmtree(tmp, ignore_errors=True)


async def run_checks(plugin):
    audible = "--audible" in sys.argv
    results = {}
    with FakeReceiver(name="SMOKE-RX") as rx:
        r = await plugin.request("settings", {"changes": {"receiver_host": "127.0.0.1", "receiver_port": rx.port,
                                                          "preset": "balanced"}})
        assert r["ok"], r
        r = await plugin.request("start")
        assert r["ok"], r
        tone = play_tone(4.0, 0.1 if audible else 0.003)
        states, levels = [], []
        for _ in range(8):
            await asyncio.sleep(0.5)
            state = (await plugin.request("state"))["data"]
            status = state["status"] or {}
            states.append(state["view"]["title"])
            levels.append((status.get("audio") or {}).get("level_db"))
        r = await plugin.request("test_sound")
        results["test_sound"] = r
        await asyncio.sleep(1.2)
        if tone:
            tone.wait(timeout=10)
        unit = subprocess.run(["systemctl", "--user", "show", "audiobridge-sender",
                               "--property=ActiveState,MainPID,ControlGroup"],
                              capture_output=True, text=True).stdout.split()
        diag = (await plugin.request("diagnostics", {"redact": True}))["data"]
        r = await plugin.request("stop")
        stopped = (await plugin.request("state"))["data"]
        results.update({
            "views": states,
            "levels_db": levels,
            "unit_while_running": unit,
            "packets_received": len(rx.audio),
            "pings_received": len(rx.pings),
            "first_header": vars(rx.audio[0][0]) if rx.audio else None,
            "chime_seen": any(max(abs(v) for v in struct.unpack("<%dh" % (len(p) // 2), p)) > 5000
                              for _, p in rx.audio[-500:]),
            "running_after_stop": stopped["running"],
            "view_after_stop": stopped["view"]["title"],
        })
    print(json.dumps(results, indent=2))
    print("\n--- diagnostics ---\n" + diag)


if __name__ == "__main__":
    asyncio.run(main())
