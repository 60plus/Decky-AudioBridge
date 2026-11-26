"""Start, stop and watch the sender engine from the plugin backend.

The engine runs as a transient systemd user unit (`systemd-run --user`), which
lives in the user's own session next to PipeWire, survives Decky reloads, and
leaves nothing installed behind once it stops. Where the user manager cannot
be reached, the engine is started as a detached process instead.
"""

from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import time
from typing import Any, Dict, List, Optional, Tuple

from .settings import PRESETS

UNIT = "audiobridge-sender"
STATUS_STALE = 3.0
SYSTEM_PATH = "/usr/local/sbin:/usr/local/bin:/usr/bin:/bin"


class EngineError(RuntimeError):
    """A user facing problem starting or stopping the engine."""


def _which(name: str) -> Optional[str]:
    return shutil.which(name, path=SYSTEM_PATH)


class EngineControl:
    def __init__(self, module_dir: str, runtime_dir: Optional[str] = None,
                 python: Optional[str] = None, uid: Optional[int] = None):
        self.module_dir = module_dir
        self.runner = os.path.join(module_dir, "audiobridge_sender.py")
        self.uid = os.getuid() if uid is None else uid
        self.runtime_dir = runtime_dir or os.environ.get("XDG_RUNTIME_DIR") or "/run/user/%d" % self.uid
        self.state_dir = os.path.join(self.runtime_dir, "audiobridge")
        self.status_file = os.path.join(self.state_dir, "status.json")
        self.pid_file = os.path.join(self.state_dir, "engine.pid")
        self.python = python or _which("python3")

    # ----- environment ---------------------------------------------------

    def env(self) -> Dict[str, str]:
        env = {
            "PATH": SYSTEM_PATH,
            "HOME": os.environ.get("HOME", "/home/deck"),
            "LANG": "C.UTF-8",
            "XDG_RUNTIME_DIR": self.runtime_dir,
            "DBUS_SESSION_BUS_ADDRESS": "unix:path=%s/bus" % self.runtime_dir,
        }
        return env

    def _run(self, args: List[str], timeout: float = 10.0) -> subprocess.CompletedProcess:
        return subprocess.run(args, env=self.env(), capture_output=True, text=True, timeout=timeout,
                              stdin=subprocess.DEVNULL)

    def _systemctl(self, *args: str, timeout: float = 10.0) -> Optional[subprocess.CompletedProcess]:
        tool = _which("systemctl")
        if not tool:
            return None
        try:
            return self._run([tool, "--user", *args], timeout=timeout)
        except (OSError, subprocess.SubprocessError):
            return None

    # ----- checks --------------------------------------------------------

    def session_ready(self) -> bool:
        """The user's runtime folder and session bus exist (PipeWire lives there)."""
        return os.path.isdir(self.runtime_dir) and os.path.exists(os.path.join(self.runtime_dir, "bus"))

    def problems(self) -> List[str]:
        """Things that would stop the engine from working, in plain words."""
        found = []
        if not self.python:
            found.append("Python 3 was not found on this system.")
        if not (_which("pw-record") or _which("parec")):
            found.append("No audio capture tool was found (pw-record or parec).")
        if not os.path.isdir(self.runtime_dir):
            found.append("The user session is not running yet.")
        return found

    # ----- state ---------------------------------------------------------

    def unit_state(self) -> Tuple[str, Optional[int]]:
        """Return (ActiveState, MainPID) of the unit, or ("unknown", None)."""
        out = self._systemctl("show", UNIT, "--property=ActiveState,MainPID", timeout=5.0)
        if out is None or out.returncode != 0:
            return "unknown", None
        values = dict(line.split("=", 1) for line in out.stdout.splitlines() if "=" in line)
        pid = values.get("MainPID", "0")
        return values.get("ActiveState", "unknown"), (int(pid) if pid.isdigit() and pid != "0" else None)

    def _fallback_pid(self) -> Optional[int]:
        try:
            with open(self.pid_file, "r", encoding="ascii") as handle:
                pid = int(handle.read().strip())
        except (OSError, ValueError):
            return None
        try:
            with open("/proc/%d/cmdline" % pid, "rb") as handle:
                if b"audiobridge_sender.py" not in handle.read():
                    return None
        except OSError:
            return None
        return pid

    def running(self) -> Tuple[bool, Optional[int]]:
        state, pid = self.unit_state()
        if state in ("active", "activating", "reloading"):
            return True, pid
        pid = self._fallback_pid()
        return (pid is not None), pid

    def read_status(self) -> Optional[Dict[str, Any]]:
        try:
            with open(self.status_file, "r", encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            return None
        if not isinstance(data, dict):
            return None
        updated = data.get("updated_at")
        data["stale"] = not isinstance(updated, (int, float)) or time.time() - updated > STATUS_STALE
        return data

    # ----- actions -------------------------------------------------------

    def command(self, settings: Dict[str, Any]) -> List[str]:
        if not self.python:
            raise EngineError("Python 3 was not found on this system.")
        if settings.get("preset") not in PRESETS:
            raise EngineError("Unknown sound preset.")
        return [self.python, "-s", "-E", self.runner,
                "--host", str(settings["receiver_host"]),
                "--port", str(int(settings["receiver_port"])),
                "--preset", settings["preset"],
                "--stream-name", settings["stream_name"],
                "--status-file", self.status_file]

    def start(self, settings: Dict[str, Any]) -> str:
        if not settings.get("receiver_host"):
            raise EngineError("Choose a receiver first.")
        problems = self.problems()
        if problems:
            raise EngineError(problems[0])
        self.stop()
        os.makedirs(self.state_dir, mode=0o700, exist_ok=True)
        try:
            os.unlink(self.status_file)
        except OSError:
            pass
        cmd = self.command(settings)
        runner = _which("systemd-run")
        if runner:
            args = [runner, "--user", "--unit", UNIT, "--collect", "--quiet",
                    "--description=Audio Bridge sender",
                    "--property=Restart=on-failure", "--property=RestartSec=2",
                    "--setenv=PYTHONDONTWRITEBYTECODE=1", "--setenv=PYTHONUNBUFFERED=1",
                    "--"] + cmd
            try:
                out = self._run(args, timeout=15.0)
            except (OSError, subprocess.SubprocessError) as exc:
                out = None
                error = str(exc)
            else:
                error = (out.stderr or out.stdout).strip()
            if out is not None and out.returncode == 0:
                return "systemd"
            # Fall through to a plain detached process.
            fallback_reason = error or "systemd-run failed"
        else:
            fallback_reason = "systemd-run not found"
        try:
            env = self.env()
            env["PYTHONDONTWRITEBYTECODE"] = "1"
            env["PYTHONUNBUFFERED"] = "1"
            log_path = os.path.join(self.state_dir, "engine.log")
            with open(log_path, "ab") as log:
                proc = subprocess.Popen(cmd, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                                        start_new_session=True, close_fds=True)
            with open(self.pid_file, "w", encoding="ascii") as handle:
                handle.write(str(proc.pid))
        except OSError as exc:
            raise EngineError("Could not start the audio engine: %s (%s)" % (exc, fallback_reason)) from None
        return "process"

    def stop(self) -> None:
        state, _ = self.unit_state()
        if state not in ("inactive", "unknown"):
            self._systemctl("stop", UNIT, timeout=15.0)
        self._systemctl("reset-failed", UNIT, timeout=5.0)
        pid = self._fallback_pid()
        if pid is not None:
            try:
                os.kill(pid, signal.SIGTERM)
                for _ in range(30):
                    time.sleep(0.1)
                    os.kill(pid, 0)
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                pass
        try:
            os.unlink(self.pid_file)
        except OSError:
            pass

    def test_sound(self) -> bool:
        running, pid = self.running()
        if not running or not pid:
            return False
        try:
            os.kill(pid, signal.SIGUSR1)
        except OSError:
            return False
        return True

    def journal(self, lines: int = 40) -> str:
        tool = _which("journalctl")
        if tool:
            try:
                out = self._run([tool, "--user", "-u", UNIT, "-n", str(lines), "--no-pager",
                                 "-o", "short-iso"], timeout=10.0)
                if out.returncode == 0 and out.stdout.strip():
                    return out.stdout.strip()
            except (OSError, subprocess.SubprocessError):
                pass
        try:
            with open(os.path.join(self.state_dir, "engine.log"), "r", encoding="utf-8",
                      errors="replace") as handle:
                return "".join(handle.readlines()[-lines:]).strip()
        except OSError:
            return ""
