"""Everything the Decky panel can ask for, independent of Decky itself."""

from __future__ import annotations

import threading
from typing import Any, Dict, List, Optional

from . import VERSION, diagnostics, discovery
from .service import EngineControl, EngineError
from .settings import PRESETS, SettingsError, SettingsStore, parse_address

RESTART_KEYS = ("receiver_host", "receiver_port", "preset", "stream_name")


class UserError(Exception):
    """Shown to the user as is."""


def receiver_label(settings: Dict[str, Any], status: Optional[Dict[str, Any]] = None) -> str:
    receiver = (status or {}).get("receiver") or {}
    name = receiver.get("name") or settings.get("receiver_name") or ""
    host = settings.get("receiver_host") or ""
    if not host:
        return ""
    address = host if settings.get("receiver_port") == 6980 else "%s:%s" % (host, settings.get("receiver_port"))
    if name and name.lower() != host.lower():
        return "%s (%s)" % (name, address)
    return address


def describe(settings: Dict[str, Any], running: bool, status: Optional[Dict[str, Any]],
             problems: List[str]) -> Dict[str, str]:
    """Turn engine state into the words the panel shows. `tone` drives colour."""
    label = receiver_label(settings, status)
    if not running:
        if status and status.get("state") not in (None, "stopped", "stopping"):
            return {"tone": "error", "title": "Streaming stopped unexpectedly",
                    "detail": "The audio engine quit. Open Diagnostics to see why, then start again."}
        if problems:
            return {"tone": "error", "title": "Cannot stream on this system", "detail": problems[0]}
        if not settings.get("receiver_host"):
            return {"tone": "idle", "title": "Not streaming",
                    "detail": "Start the receiver on your other PC, then choose it below."}
        return {"tone": "idle", "title": "Not streaming", "detail": "Ready to stream to %s." % label}
    if not status or status.get("state") in (None, "stopped"):
        return {"tone": "busy", "title": "Starting", "detail": "Starting the audio engine."}
    if status.get("stale"):
        return {"tone": "warn", "title": "Audio engine is not responding",
                "detail": "Stop streaming and start it again. If it keeps happening, open Diagnostics."}
    state = status.get("state")
    receiver = status.get("receiver") or {}
    ip = receiver.get("ip") or settings.get("receiver_host")
    if state == "streaming":
        rtt = receiver.get("rtt_ms")
        detail = "To %s." % label
        if rtt is not None:
            detail += " The receiver answers in %s ms." % (rtt if rtt >= 1 else "under 1")
        return {"tone": "ok", "title": "Streaming", "detail": detail}
    if state == "connecting":
        return {"tone": "busy", "title": "Connecting", "detail": "Contacting %s." % label}
    if state == "no_reply":
        return {"tone": "warn", "title": "Receiver is not answering",
                "detail": "Audio is being sent to %s, but nothing answers. Check that Audio Bridge Receiver "
                          "is running on that PC and that Windows Firewall allows it." % ip}
    if state == "receiver_not_found":
        return {"tone": "error", "title": "Receiver not found",
                "detail": "%s could not be found on the network. Try its IP address instead "
                          "(the receiver window shows it)." % settings.get("receiver_host")}
    if state == "network_error":
        return {"tone": "error", "title": "Network problem",
                "detail": "%s. Streaming continues as soon as the network is back." % status.get("detail", "")}
    if state == "capture_lost":
        return {"tone": "warn", "title": "Waiting for system audio",
                "detail": "The audio system is not delivering sound right now. Streaming resumes "
                          "automatically. (%s)" % status.get("detail", "")}
    return {"tone": "busy", "title": "Working", "detail": status.get("detail") or ""}


class Backend:
    def __init__(self, settings_dir: str, module_dir: str, runtime_dir: Optional[str] = None,
                 control: Optional[EngineControl] = None):
        self.store = SettingsStore(settings_dir)
        self.control = control or EngineControl(module_dir, runtime_dir)
        self.lock = threading.RLock()

    # ----- reading -------------------------------------------------------

    def state(self) -> Dict[str, Any]:
        with self.lock:
            settings = self.store.load()
            running, _pid = self.control.running()
            status = self.control.read_status()
            problems = self.control.problems()
            view = describe(settings, running, status, problems)
            return {
                "version": VERSION,
                "settings": settings,
                "running": running,
                "status": status if running else None,
                "view": view,
                "receiver_label": receiver_label(settings, status if running else None),
                "presets": [{"id": key, "label": value["label"], "buffer_ms": value["buffer_ms"]}
                            for key, value in PRESETS.items()],
            }

    def diagnostics(self, redact: bool = True) -> str:
        with self.lock:
            settings = self.store.load()
            running, _ = self.control.running()
            status = self.control.read_status()
            return diagnostics.report(VERSION, settings, running, status, self.control.problems(),
                                      self.control.journal(), redact=redact)

    # ----- actions -------------------------------------------------------

    def start(self) -> Dict[str, Any]:
        with self.lock:
            settings = self.store.load()
            try:
                self.control.start(settings)
            except EngineError as exc:
                raise UserError(str(exc)) from None
            return self.state()

    def stop(self) -> Dict[str, Any]:
        with self.lock:
            self.control.stop()
            return self.state()

    def autostart(self) -> Optional[bool]:
        """Called when the plugin loads. Leaves a running engine alone.

        Returns True when it started the stream, False when there is nothing
        to do, and None when the user session is not up yet (right after boot
        Decky can load before it) so the caller should try again shortly.
        """
        with self.lock:
            settings = self.store.load()
            if not settings.get("auto_start") or not settings.get("receiver_host"):
                return False
            if not self.control.session_ready():
                return None
            running, _ = self.control.running()
            if running:
                return False
            try:
                self.control.start(settings)
            except EngineError:
                return None
            return True

    def update_settings(self, changes: Dict[str, Any]) -> Dict[str, Any]:
        with self.lock:
            before = self.store.load()
            try:
                after = self.store.update(changes)
            except SettingsError as exc:
                raise UserError(str(exc)) from None
            running, _ = self.control.running()
            if running and any(before.get(k) != after.get(k) for k in RESTART_KEYS):
                if after.get("receiver_host"):
                    try:
                        self.control.start(after)
                    except EngineError as exc:
                        raise UserError(str(exc)) from None
                else:
                    self.control.stop()
            return self.state()

    def set_address(self, text: str) -> Dict[str, Any]:
        try:
            host, port = parse_address(text)
        except SettingsError as exc:
            raise UserError(str(exc)) from None
        if not host:
            raise UserError("Enter the receiver's IP address, for example 192.168.0.40.")
        changes: Dict[str, Any] = {"receiver_host": host, "receiver_name": ""}
        if port is not None:
            changes["receiver_port"] = port
        return self.update_settings(changes)

    def choose_receiver(self, ip: str, port: int, name: str = "") -> Dict[str, Any]:
        return self.update_settings({"receiver_host": ip, "receiver_port": port, "receiver_name": name or ""})

    def discover(self) -> List[Dict[str, Any]]:
        settings = self.store.load()
        extra = [settings["receiver_host"]] if settings.get("receiver_host") else []
        return discovery.discover(port=settings.get("receiver_port") or 6980, extra_hosts=extra)

    def test_sound(self) -> bool:
        with self.lock:
            if not self.control.test_sound():
                raise UserError("Start streaming first, then play the test sound.")
            return True

    def uninstall(self) -> None:
        with self.lock:
            self.control.stop()
