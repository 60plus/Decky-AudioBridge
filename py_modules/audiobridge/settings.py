"""Plugin settings: defaults, validation and atomic persistence."""

from __future__ import annotations

import ipaddress
import json
import os
import re
import tempfile
from typing import Any, Dict, Optional, Tuple

from . import vban

SCHEMA_VERSION = 1

# Each preset decides the sender side (packet size, sample format, how often
# PipeWire hands us audio) and suggests a playback buffer to the receiver.
PRESETS: Dict[str, Dict[str, Any]] = {
    "low": {
        "label": "Low latency",
        "samples": 64,
        "datatype": vban.DATATYPE_INT16,
        "capture_latency": 256,
        "buffer_ms": 20,
    },
    "balanced": {
        "label": "Balanced",
        "samples": 128,
        "datatype": vban.DATATYPE_INT16,
        "capture_latency": 512,
        "buffer_ms": 40,
    },
    "high": {
        "label": "High quality",
        "samples": 128,
        "datatype": vban.DATATYPE_INT24,
        "capture_latency": 1024,
        "buffer_ms": 80,
    },
}

DEFAULTS: Dict[str, Any] = {
    "version": SCHEMA_VERSION,
    "receiver_host": "",
    "receiver_port": vban.DEFAULT_PORT,
    "receiver_name": "",
    "preset": "balanced",
    "auto_start": False,
    "stream_name": "AudioBridge",
}

_HOST_LABEL = re.compile(r"^(?!-)[A-Za-z0-9-]{1,63}(?<!-)$")
_STREAM_NAME = re.compile(r"^[A-Za-z0-9 _.\-]{1,16}$")


class SettingsError(ValueError):
    """A setting has a value the plugin cannot use. The message is user facing."""


def validate_host(value: Any) -> str:
    if not isinstance(value, str):
        raise SettingsError("Receiver address must be text.")
    host = value.strip()
    if not host:
        return ""
    if len(host) > 253:
        raise SettingsError("Receiver address is too long.")
    try:
        address = ipaddress.ip_address(host)
    except ValueError:
        address = None
    if address is not None:
        if address.version != 4:
            raise SettingsError("Use an IPv4 address such as 192.168.0.40.")
        if address.is_multicast or address.is_unspecified:
            raise SettingsError("Use the address of one PC, not a group address.")
        return host
    labels = host.rstrip(".").split(".")
    if all(label.isdigit() for label in labels):
        raise SettingsError("That is not a valid IPv4 address.")
    if not all(_HOST_LABEL.match(label) for label in labels):
        raise SettingsError("Receiver address is not a valid IP address or computer name.")
    return host


def validate_port(value: Any) -> int:
    if isinstance(value, bool):
        raise SettingsError("Port must be a number.")
    try:
        port = int(value)
    except (TypeError, ValueError):
        raise SettingsError("Port must be a number.") from None
    if not 1024 <= port <= 65535:
        raise SettingsError("Port must be between 1024 and 65535.")
    return port


def parse_address(text: Any) -> Tuple[str, Optional[int]]:
    """Split user input like `192.168.0.40` or `work-pc:6980`."""
    if not isinstance(text, str):
        raise SettingsError("Receiver address must be text.")
    text = text.strip()
    host, sep, port = text.rpartition(":")
    if sep and port.isdigit():
        return validate_host(host), validate_port(port)
    return validate_host(text), None


def validate(raw: Dict[str, Any]) -> Dict[str, Any]:
    """Return a complete, validated settings dict. Unknown keys are dropped."""
    out = dict(DEFAULTS)
    if not isinstance(raw, dict):
        return out
    if "receiver_host" in raw:
        out["receiver_host"] = validate_host(raw["receiver_host"])
    if "receiver_port" in raw:
        out["receiver_port"] = validate_port(raw["receiver_port"])
    if "receiver_name" in raw:
        name = raw["receiver_name"]
        if not isinstance(name, str):
            raise SettingsError("Receiver name must be text.")
        out["receiver_name"] = "".join(ch for ch in name if ch.isprintable())[:64]
    if "preset" in raw:
        if raw["preset"] not in PRESETS:
            raise SettingsError("Unknown sound preset.")
        out["preset"] = raw["preset"]
    if "auto_start" in raw:
        if not isinstance(raw["auto_start"], bool):
            raise SettingsError("Start automatically must be on or off.")
        out["auto_start"] = raw["auto_start"]
    if "stream_name" in raw:
        name = raw["stream_name"]
        if not isinstance(name, str) or not _STREAM_NAME.match(name):
            raise SettingsError("Stream name must be 1 to 16 letters, digits, spaces, dots, dashes or underscores.")
        out["stream_name"] = name
    out["version"] = SCHEMA_VERSION
    return out


def _migrate(raw: Dict[str, Any]) -> Dict[str, Any]:
    """Bring older settings files up to the current schema."""
    if not isinstance(raw, dict):
        return {}
    # Version 1 is the first schema; later versions add steps here.
    return raw


def _salvage(raw: Dict[str, Any]) -> Dict[str, Any]:
    """Keep every individually valid value from a file with some bad ones."""
    out = dict(DEFAULTS)
    for key in DEFAULTS:
        if key in raw and key != "version":
            try:
                out[key] = validate({key: raw[key]})[key]
            except SettingsError:
                pass
    return out


class SettingsStore:
    def __init__(self, directory: str):
        self.directory = directory
        self.path = os.path.join(directory, "settings.json")

    def load(self) -> Dict[str, Any]:
        try:
            with open(self.path, "r", encoding="utf-8") as handle:
                raw = json.load(handle)
        except FileNotFoundError:
            return dict(DEFAULTS)
        except (OSError, ValueError):
            return dict(DEFAULTS)
        raw = _migrate(raw)
        try:
            return validate(raw)
        except SettingsError:
            return _salvage(raw)

    def save(self, settings: Dict[str, Any]) -> Dict[str, Any]:
        clean = validate(settings)
        os.makedirs(self.directory, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".settings-", suffix=".json", dir=self.directory)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as handle:
                json.dump(clean, handle, indent=2, sort_keys=True)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(tmp, self.path)
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
        return clean

    def update(self, changes: Dict[str, Any]) -> Dict[str, Any]:
        if not isinstance(changes, dict):
            raise SettingsError("Settings change must be an object.")
        unknown = set(changes) - set(DEFAULTS) - {"version"}
        if unknown:
            raise SettingsError("Unknown setting: " + ", ".join(sorted(unknown)))
        merged = self.load()
        merged.update(changes)
        return self.save(merged)
