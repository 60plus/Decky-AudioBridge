"""Find receivers on the local network with a VBAN PING0 broadcast.

Discovery only runs when the user asks for it. The request carries the
plugin name and this computer's host name, nothing else.
"""

from __future__ import annotations

import ipaddress
import json
import shutil
import socket
import subprocess
import time
from typing import Dict, Iterable, List, Optional

from . import APP_NAME, VERSION, vban

SYSTEM_PATH = "/usr/local/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"


def broadcast_addresses() -> List[str]:
    """Directed broadcast address of every IPv4 network this machine is on."""
    found: List[str] = []
    tool = shutil.which("ip", path=SYSTEM_PATH)
    if tool:
        try:
            out = subprocess.run([tool, "-j", "-4", "addr", "show", "up"], capture_output=True,
                                 text=True, timeout=5)
            for link in json.loads(out.stdout or "[]"):
                if "LOOPBACK" in link.get("flags", []):
                    continue
                for addr in link.get("addr_info", []):
                    if addr.get("family") != "inet":
                        continue
                    broadcast = addr.get("broadcast")
                    if not broadcast and addr.get("local") and addr.get("prefixlen") is not None:
                        try:
                            net = ipaddress.IPv4Network("%s/%s" % (addr["local"], addr["prefixlen"]), strict=False)
                            broadcast = str(net.broadcast_address)
                        except ValueError:
                            broadcast = None
                    if broadcast and broadcast not in found:
                        found.append(broadcast)
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
    if "255.255.255.255" not in found:
        found.append("255.255.255.255")
    return found


def _identity() -> vban.Identity:
    return vban.Identity(
        device_type=vban.TYPE_TRANSMITTER, features=vban.FEATURE_AUDIO,
        version=tuple(int(x) for x in VERSION.split(".")[:3]) + (0,),
        device_name="System audio", manufacturer="60plus", application=APP_NAME,
        host_name=socket.gethostname()[:63],
    )


def discover(port: int = vban.DEFAULT_PORT, timeout: float = 1.5,
             addresses: Optional[Iterable[str]] = None,
             extra_hosts: Iterable[str] = ()) -> List[Dict[str, object]]:
    """Broadcast a PING0 and list everything that answers as a receiver."""
    targets = list(addresses) if addresses is not None else broadcast_addresses()
    targets += [h for h in extra_hosts if h and h not in targets]
    identity = _identity()
    results: Dict[tuple, Dict[str, object]] = {}
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_BROADCAST, 1)
        sock.bind(("0.0.0.0", 0))
        sock.settimeout(0.1)
        frame = int(time.time()) & 0xFFFF
        frames = set()
        deadline = time.monotonic() + timeout
        next_send = time.monotonic()
        rounds = 0
        while time.monotonic() < deadline:
            now = time.monotonic()
            if rounds < 2 and now >= next_send:
                frame += 1
                frames.add(frame)
                packet = vban.ping_packet(identity, frame)
                for target in targets:
                    try:
                        sock.sendto(packet, (target, port))
                    except OSError:
                        pass
                rounds += 1
                next_send = now + 0.5
            try:
                data, addr = sock.recvfrom(2048)
            except socket.timeout:
                continue
            except OSError:
                continue
            parsed = vban.parse_ping(data)
            if not parsed:
                continue
            is_reply, reply_frame, ident = parsed
            if not is_reply or reply_frame not in frames:
                continue
            if not ident.device_type & vban.TYPE_CAN_RECEIVE:
                continue
            key = (addr[0], addr[1])
            hints = vban.parse_comment(ident.comment)
            results[key] = {
                "ip": addr[0],
                "port": addr[1],
                "name": ident.host_name or addr[0],
                "device": ident.device_name,
                "application": ident.application,
                "ours": ident.application.startswith(APP_NAME) or hints.get("audiobridge") == "1",
            }
    finally:
        sock.close()
    return sorted(results.values(), key=lambda r: (not r["ours"], str(r["name"]).lower(), r["ip"]))
