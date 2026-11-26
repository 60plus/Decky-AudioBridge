"""A plain text report for troubleshooting.

It never contains audio, credentials, the user name or full command lines.
Receiver addresses can be hidden with `redact`.
"""

from __future__ import annotations

import os
import platform
import re
import shutil
import subprocess
import sys
from typing import Any, Dict, List, Optional

SYSTEM_PATH = "/usr/local/bin:/usr/bin:/bin"
_IPV4 = re.compile(r"\b(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})\b")


def _os_release() -> str:
    values: Dict[str, str] = {}
    try:
        with open("/etc/os-release", "r", encoding="utf-8") as handle:
            for line in handle:
                key, sep, value = line.strip().partition("=")
                if sep:
                    values[key] = value.strip('"')
    except OSError:
        return "unknown"
    parts = [values.get("NAME", "Linux"), values.get("VERSION_ID", "")]
    if values.get("BUILD_ID"):
        parts.append("build " + values["BUILD_ID"])
    return " ".join(p for p in parts if p)


def _tool_version(name: str, args: List[str]) -> Optional[str]:
    tool = shutil.which(name, path=SYSTEM_PATH)
    if not tool:
        return None
    try:
        out = subprocess.run([tool] + args, capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return None
    text = (out.stdout or out.stderr).strip().splitlines()
    return text[-1].strip() if text else None


def redact_text(text: str) -> str:
    """Keep the first two octets of private addresses, hide the rest."""
    def repl(match):
        a, b = match.group(1), match.group(2)
        return "%s.%s.x.x" % (a, b)
    text = _IPV4.sub(repl, text)
    home = os.path.expanduser("~")
    if home and home != "/":
        text = text.replace(home, "~")
    return text


def report(plugin_version: str, settings: Dict[str, Any], running: bool,
           status: Optional[Dict[str, Any]], problems: List[str], journal: str,
           redact: bool = True) -> str:
    lines = [
        "Audio Bridge diagnostics",
        "Plugin version: %s" % plugin_version,
        "Engine version: %s" % ((status or {}).get("engine_version") or "not running"),
        "System: %s" % _os_release(),
        "Kernel: %s" % platform.release(),
        "Backend Python: %s" % sys.version.split()[0],
        "PipeWire: %s" % (_tool_version("pipewire", ["--version"]) or "not found"),
        "pw-record: %s" % ("found" if shutil.which("pw-record", path=SYSTEM_PATH) else "not found"),
        "parec: %s" % ("found" if shutil.which("parec", path=SYSTEM_PATH) else "not found"),
        "",
        "Receiver: %s:%s" % (settings.get("receiver_host") or "(not set)", settings.get("receiver_port")),
        "Preset: %s" % settings.get("preset"),
        "Start automatically: %s" % ("yes" if settings.get("auto_start") else "no"),
        "Engine running: %s" % ("yes" if running else "no"),
    ]
    if problems:
        lines.append("Problems: " + "; ".join(problems))
    if status:
        receiver = status.get("receiver") or {}
        audio = status.get("audio") or {}
        net = status.get("network") or {}
        lines += [
            "State: %s (%s)" % (status.get("state"), status.get("detail")),
            "Receiver answering: %s%s" % (
                "yes" if receiver.get("answering") else "no",
                ", %s ms" % receiver.get("rtt_ms") if receiver.get("rtt_ms") is not None else ""),
            "Receiver application: %s" % (receiver.get("application") or "unknown"),
            "Audio: %s Hz, %s channels, %s, %s samples per packet, capture %s, restarts %s" % (
                audio.get("rate"), audio.get("channels"), audio.get("format"),
                audio.get("samples_per_packet"), audio.get("capture"), audio.get("capture_restarts")),
            "Level: %s dBFS" % (audio.get("level_db") if audio.get("level_db") is not None else "silent"),
            "Packets sent: %s, dropped: %s, send errors: %s, last error: %s" % (
                net.get("packets"), net.get("dropped"), net.get("send_errors"), net.get("last_error") or "none"),
        ]
    if journal:
        lines += ["", "Recent engine log:", journal]
    text = "\n".join(lines)
    return redact_text(text) if redact else text
