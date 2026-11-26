"""The sender engine.

It runs in its own process, normally as a transient systemd user unit started
by the plugin backend, so closing the Decky panel or reloading the plugin does
not interrupt the stream. It captures the monitor of the default audio output
with pw-record (parec as a fallback), cuts the PCM into VBAN packets, sends them
over UDP and pings the receiver once a second to learn whether it is listening.
Its state is written to a small JSON file the backend reads.
"""

from __future__ import annotations

import argparse
import errno
import ipaddress
import json
import logging
import math
import os
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
from array import array
from typing import List, Optional, Tuple

from . import APP_NAME, VERSION, vban
from .settings import PRESETS

RATE = 48000
CHANNELS = 2
PING_INTERVAL = 1.0
REPLY_TIMEOUT = 3.5
STATUS_INTERVAL = 0.5
RESOLVE_INTERVAL = 30.0
CAPTURE_STALL = 3.0
NETWORK_ERROR_HOLD = 3.0
SYSTEM_PATH = "/usr/bin:/bin:/usr/local/bin"

# datatype -> (pw-record format, parec format, bytes per sample, label)
FORMATS = {
    vban.DATATYPE_INT16: ("s16", "s16le", 2, "16-bit"),
    vban.DATATYPE_INT24: ("s24", "s24le", 3, "24-bit"),
}

log = logging.getLogger("audiobridge")


def find_tool(name: str) -> Optional[str]:
    path = os.environ.get("PATH", "")
    return shutil.which(name, path=(path + ":" if path else "") + SYSTEM_PATH)


def pw_record_supports_raw(path: str) -> bool:
    try:
        out = subprocess.run([path, "--help"], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.SubprocessError):
        return False
    return "--raw" in (out.stdout + out.stderr)


def capture_command(datatype: int, latency: int) -> Tuple[Optional[List[str]], str]:
    """Pick the capture tool. Nothing here changes the system's audio routing:
    the capture stream links to the monitor of whatever output is the default
    and follows it when the default changes."""
    pw_fmt, pa_fmt, _, _ = FORMATS[datatype]
    pw = find_tool("pw-record")
    if pw and pw_record_supports_raw(pw):
        props = ('{ stream.capture.sink = true node.name = audiobridge-capture '
                 'node.description = "Audio Bridge" media.name = "Audio Bridge capture" }')
        return [pw, "--raw", "-P", props, "--format", pw_fmt, "--rate", str(RATE),
                "--channels", str(CHANNELS), "--latency", str(latency), "-"], "pw-record"
    pa = find_tool("parec")
    if pa:
        ms = max(5, int(latency * 1000 / RATE))
        return [pa, "--device=@DEFAULT_MONITOR@", "--raw", "--format=" + pa_fmt,
                "--rate=%d" % RATE, "--channels=%d" % CHANNELS, "--latency-msec=%d" % ms,
                "--client-name=Audio Bridge", "--stream-name=Audio Bridge capture"], "parec"
    return None, ""


def encode_samples(values: List[float], datatype: int) -> bytes:
    """Float samples in -1..1 to interleaved little-endian PCM."""
    if datatype == vban.DATATYPE_INT16:
        return struct.pack("<%dh" % len(values),
                           *(max(-32768, min(32767, int(v * 32767))) for v in values))
    out = bytearray()
    for v in values:
        out += max(-8388608, min(8388607, int(v * 8388607))).to_bytes(3, "little", signed=True)
    return bytes(out)


def make_chime(datatype: int) -> bytes:
    """A short two-note test sound, stereo."""
    notes = ((0.0, 0.32, 659.25), (0.36, 0.86, 880.0))
    total = int(RATE * 0.9)
    values: List[float] = []
    for i in range(total):
        t = i / RATE
        v = 0.0
        for start, end, freq in notes:
            if start <= t < end:
                x = t - start
                env = min(1.0, x / 0.01, (end - start - x) / 0.12)
                v += 0.25 * env * math.sin(2 * math.pi * freq * x)
        values.append(v)
        values.append(v)
    return encode_samples(values, datatype)


def peak_of(chunk: bytes, datatype: int) -> float:
    if not chunk:
        return 0.0
    if datatype == vban.DATATYPE_INT16:
        samples = array("h")
        samples.frombytes(chunk[: len(chunk) - len(chunk) % 2])
        if sys.byteorder != "little":
            samples.byteswap()
        if not samples:
            return 0.0
        return max(max(samples), -min(samples)) / 32768.0
    msb = array("b", chunk[2::3])
    if not msb:
        return 0.0
    return max(max(msb), -min(msb)) / 128.0


def is_ip(host: str) -> bool:
    try:
        ipaddress.IPv4Address(host)
        return True
    except ValueError:
        return False


class Engine:
    def __init__(self, host: str, port: int, preset: str, stream_name: str,
                 status_file: str, source: str = "system", duration: float = 0.0):
        if preset not in PRESETS:
            raise ValueError("unknown preset")
        p = PRESETS[preset]
        self.host = host
        self.port = port
        self.preset = preset
        self.stream_name = stream_name
        self.status_file = status_file
        self.source = source
        self.duration = duration
        self.datatype = p["datatype"]
        self.sample_bytes = FORMATS[self.datatype][2]
        self.samples = min(p["samples"], vban.max_samples(CHANNELS, self.datatype))
        self.packet_bytes = self.samples * CHANNELS * self.sample_bytes
        self.capture_latency = p["capture_latency"]
        self.header_prefix = vban.audio_header(RATE, self.samples, CHANNELS, self.datatype,
                                               stream_name, 0)[:24]

        self.stopping = False
        self.chime: Optional[bytes] = None
        self.chime_pos = -1
        self.chime_requested = False
        self.frame = 0

        self.target: Optional[Tuple[str, int]] = (host, port) if is_ip(host) else None
        self.resolve_error: Optional[str] = None
        self._resolve_lock = threading.Lock()

        self.capture: Optional[subprocess.Popen] = None
        self.capture_tool = ""
        self.capture_restarts = 0
        self.capture_next_try = 0.0
        self.capture_backoff = 1.0
        self.capture_error: Optional[str] = None
        self.last_data = 0.0
        self.buffer = bytearray()
        self.tone_phase = 0.0
        self.tone_produced = 0

        self.ping_frame = 0
        self.pending_pings = {}
        self.last_reply = 0.0
        self.rtt_ms: Optional[float] = None
        self.receiver_identity: Optional[vban.Identity] = None

        self.packets = 0
        self.bytes_sent = 0
        self.dropped = 0
        self.send_errors = 0
        self.last_error: Optional[str] = None
        self.last_error_at = 0.0
        self.peak_window = 0.0
        self.level_db: Optional[float] = None

        self.started = time.monotonic()
        self.started_wall = time.time()

        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.setblocking(False)
        try:
            self.sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 1 << 18)
        except OSError:
            pass
        try:
            # Ask the network stack to treat audio as interactive traffic.
            self.sock.setsockopt(socket.IPPROTO_IP, socket.IP_TOS, 0xB8)
        except (OSError, AttributeError):
            pass

        hostname = socket.gethostname()[:63] or "SteamOS"
        self.identity = vban.Identity(
            device_type=vban.TYPE_TRANSMITTER, features=vban.FEATURE_AUDIO,
            version=tuple(int(x) for x in VERSION.split(".")[:3]) + (0,),
            device_name="System audio", manufacturer="60plus", application=APP_NAME,
            host_name=hostname,
            comment="audiobridge=1;preset=%s;buffer=%d" % (preset, p["buffer_ms"]),
        )

    # ----- control -------------------------------------------------------

    def request_stop(self, *_args) -> None:
        self.stopping = True

    def request_chime(self, *_args) -> None:
        self.chime_requested = True

    # ----- receiver address ----------------------------------------------

    def _resolver(self) -> None:
        while not self.stopping:
            try:
                infos = socket.getaddrinfo(self.host, self.port, socket.AF_INET, socket.SOCK_DGRAM)
                ip = infos[0][4][0]
                with self._resolve_lock:
                    if self.target is None or self.target[0] != ip:
                        log.info("receiver %s resolved to %s", self.host, ip)
                    self.target = (ip, self.port)
                    self.resolve_error = None
            except (OSError, IndexError) as exc:
                with self._resolve_lock:
                    self.resolve_error = str(exc)
                    if self.target is None:
                        log.warning("cannot resolve %s: %s", self.host, exc)
            deadline = time.monotonic() + (RESOLVE_INTERVAL if self.target else 3.0)
            while not self.stopping and time.monotonic() < deadline:
                time.sleep(0.2)

    # ----- capture -------------------------------------------------------

    def _start_capture(self, now: float) -> None:
        cmd, tool = capture_command(self.datatype, self.capture_latency)
        if not cmd:
            self.capture_error = "No audio capture tool found (pw-record or parec)."
            self.capture_next_try = now + 10.0
            return
        try:
            self.capture = subprocess.Popen(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                            stderr=None, bufsize=0, close_fds=True)
        except OSError as exc:
            self.capture = None
            self.capture_error = "Could not start %s: %s" % (tool, exc)
            self.capture_next_try = now + self.capture_backoff
            self.capture_backoff = min(5.0, self.capture_backoff * 2)
            return
        self.capture_tool = tool
        self.capture_error = None
        self.last_data = now
        self.buffer.clear()
        log.info("capture started with %s", tool)

    def _stop_capture(self) -> None:
        proc, self.capture = self.capture, None
        if proc is None:
            return
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=1.5)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=1.5)
        if proc.stdout:
            proc.stdout.close()

    def _capture_lost(self, now: float, reason: str) -> None:
        log.warning("capture lost: %s", reason)
        self._stop_capture()
        self.capture_error = reason
        self.capture_restarts += 1
        self.capture_next_try = now + self.capture_backoff
        self.capture_backoff = min(5.0, self.capture_backoff * 2)

    def _tone(self, now: float) -> None:
        due = int((now - self.started) * RATE) - self.tone_produced
        if due <= 0:
            return
        due = min(due, RATE // 4)
        values = []
        step = 2 * math.pi * 440.0 / RATE
        for _ in range(due):
            v = 0.2 * math.sin(self.tone_phase)
            self.tone_phase += step
            values.append(v)
            values.append(v)
        self.tone_phase %= 2 * math.pi
        self.tone_produced += due
        self.buffer += encode_samples(values, self.datatype)
        self.last_data = now

    # ----- sending -------------------------------------------------------

    def _send_packets(self, now: float) -> None:
        size = self.packet_bytes
        while len(self.buffer) >= size:
            payload = bytes(self.buffer[:size])
            del self.buffer[:size]
            self.peak_window = max(self.peak_window, peak_of(payload, self.datatype))
            if self.chime_pos >= 0 and self.chime is not None:
                part = self.chime[self.chime_pos:self.chime_pos + size]
                self.chime_pos += size
                if len(part) < size:
                    part += b"\0" * (size - len(part))
                    self.chime_pos = -1
                payload = part
            packet = self.header_prefix + struct.pack("<I", self.frame) + payload
            self.frame = (self.frame + 1) & 0xFFFFFFFF
            target = self.target
            if target is None:
                self.dropped += 1
                continue
            try:
                self.sock.sendto(packet, target)
                self.packets += 1
                self.bytes_sent += len(packet)
            except BlockingIOError:
                self.dropped += 1
            except OSError as exc:
                self._network_error(now, exc)

    def _network_error(self, now: float, exc: OSError) -> None:
        self.send_errors += 1
        if exc.errno == errno.ECONNREFUSED:
            return
        message = exc.strerror or str(exc)
        if message != self.last_error or now - self.last_error_at > 10:
            log.warning("send failed: %s", message)
        self.last_error = message
        self.last_error_at = now

    def _ping(self, now: float) -> None:
        target = self.target
        if target is None:
            return
        self.ping_frame = (self.ping_frame + 1) & 0xFFFFFFFF
        self.pending_pings[self.ping_frame] = now
        if len(self.pending_pings) > 8:
            for key in sorted(self.pending_pings, key=self.pending_pings.get)[:-8]:
                del self.pending_pings[key]
        try:
            self.sock.sendto(vban.ping_packet(self.identity, self.ping_frame), target)
        except BlockingIOError:
            pass
        except OSError as exc:
            self._network_error(now, exc)

    def _read_replies(self, now: float) -> None:
        while True:
            try:
                data, addr = self.sock.recvfrom(2048)
            except (BlockingIOError, InterruptedError):
                return
            except OSError as exc:
                if exc.errno in (errno.ECONNREFUSED, errno.EHOSTUNREACH, errno.ENETUNREACH):
                    continue
                return
            target = self.target
            if target is None or addr[0] != target[0]:
                continue
            parsed = vban.parse_ping(data)
            if not parsed:
                continue
            is_reply, frame, identity = parsed
            if not is_reply:
                continue
            sent = self.pending_pings.pop(frame, None)
            if sent is not None:
                self.rtt_ms = round((now - sent) * 1000.0, 1)
            if self.last_reply == 0.0:
                log.info("receiver answered: %s (%s)", identity.host_name or addr[0],
                         identity.application or "unknown application")
            self.last_reply = now
            self.receiver_identity = identity

    # ----- status --------------------------------------------------------

    def state(self, now: float) -> Tuple[str, str]:
        if self.stopping:
            return "stopping", "Stopping"
        if self.source == "system":
            alive = self.capture is not None and self.capture.poll() is None
            if not alive or now - self.last_data > CAPTURE_STALL:
                return "capture_lost", self.capture_error or "Waiting for system audio"
        if self.target is None:
            if self.resolve_error:
                return "receiver_not_found", "Cannot find %s on the network" % self.host
            return "connecting", "Looking up %s" % self.host
        if self.last_error and now - self.last_error_at < NETWORK_ERROR_HOLD:
            return "network_error", "Network problem: %s" % self.last_error
        if self.last_reply and now - self.last_reply < REPLY_TIMEOUT:
            return "streaming", "Streaming"
        if now - self.started < REPLY_TIMEOUT:
            return "connecting", "Contacting the receiver"
        return "no_reply", "Sending audio, but the receiver is not answering"

    def status(self, now: float) -> dict:
        state, detail = self.state(now)
        ident = self.receiver_identity
        answering = bool(self.last_reply and now - self.last_reply < REPLY_TIMEOUT)
        return {
            "engine_version": VERSION,
            "pid": os.getpid(),
            "state": state,
            "detail": detail,
            "preset": self.preset,
            "receiver": {
                "host": self.host,
                "ip": self.target[0] if self.target else None,
                "port": self.port,
                "answering": answering,
                "name": (ident.host_name or None) if ident else None,
                "application": (ident.application or None) if ident else None,
                "device": (ident.device_name or None) if ident else None,
                "rtt_ms": self.rtt_ms if answering else None,
            },
            "audio": {
                "rate": RATE,
                "channels": CHANNELS,
                "format": FORMATS[self.datatype][3],
                "samples_per_packet": self.samples,
                "packet_ms": round(self.samples * 1000.0 / RATE, 2),
                "capture": self.capture_tool if self.source == "system" else "test tone",
                "capture_restarts": self.capture_restarts,
                "level_db": self.level_db,
            },
            "network": {
                "packets": self.packets,
                "bytes": self.bytes_sent,
                "dropped": self.dropped,
                "send_errors": self.send_errors,
                "last_error": self.last_error,
            },
            "started_at": self.started_wall,
            "updated_at": time.time(),
        }

    def write_status(self, now: float) -> None:
        if self.peak_window > 0:
            self.level_db = round(20 * math.log10(self.peak_window), 1)
        else:
            self.level_db = None
        self.peak_window = 0.0
        write_json_atomic(self.status_file, self.status(now))

    # ----- main loop -----------------------------------------------------

    def run(self) -> int:
        signal.signal(signal.SIGTERM, self.request_stop)
        signal.signal(signal.SIGINT, self.request_stop)
        if hasattr(signal, "SIGUSR1"):
            signal.signal(signal.SIGUSR1, self.request_chime)
        if hasattr(signal, "SIGHUP"):
            signal.signal(signal.SIGHUP, signal.SIG_IGN)
        if self.target is None:
            threading.Thread(target=self._resolver, name="resolver", daemon=True).start()
        log.info("engine %s started: %s:%d, preset %s, %d samples per packet, %s",
                 VERSION, self.host, self.port, self.preset, self.samples, FORMATS[self.datatype][3])
        next_ping = time.monotonic()
        next_status = time.monotonic()
        deadline = self.started + self.duration if self.duration > 0 else None
        try:
            while not self.stopping:
                now = time.monotonic()
                if deadline and now >= deadline:
                    break
                if self.chime_requested:
                    self.chime_requested = False
                    if self.chime is None:
                        self.chime = make_chime(self.datatype)
                    self.chime_pos = 0
                if self.source == "system" and self.capture is None and now >= self.capture_next_try:
                    self._start_capture(now)
                readers = [self.sock]
                if self.capture is not None and self.capture.stdout is not None:
                    readers.append(self.capture.stdout)
                timeout = 0.005 if self.source == "tone" else 0.2
                try:
                    ready, _, _ = select.select(readers, [], [], timeout)
                except InterruptedError:
                    continue
                now = time.monotonic()
                if self.capture is not None and self.capture.stdout in ready:
                    try:
                        data = os.read(self.capture.stdout.fileno(), 65536)
                    except OSError as exc:
                        data = b""
                        self.capture_error = str(exc)
                    if data:
                        self.buffer += data
                        self.last_data = now
                        self.capture_backoff = 1.0
                    else:
                        code = self.capture.poll()
                        self._capture_lost(now, "%s stopped (exit code %s)" % (self.capture_tool, code))
                elif self.capture is not None and now - self.last_data > CAPTURE_STALL:
                    self._capture_lost(now, "%s stopped delivering audio" % self.capture_tool)
                if self.source == "tone":
                    self._tone(now)
                if self.buffer:
                    self._send_packets(now)
                if self.sock in ready:
                    self._read_replies(now)
                if now >= next_ping:
                    self._ping(now)
                    next_ping = now + PING_INTERVAL
                if now >= next_status:
                    self.write_status(now)
                    next_status = now + STATUS_INTERVAL
        finally:
            self.stopping = True
            self._stop_capture()
            try:
                status = self.status(time.monotonic())
                status["state"] = "stopped"
                status["detail"] = "Stopped"
                write_json_atomic(self.status_file, status)
            except OSError:
                pass
            self.sock.close()
            log.info("engine stopped after %d packets", self.packets)
        return 0


def write_json_atomic(path: str, data: dict) -> None:
    directory = os.path.dirname(path) or "."
    os.makedirs(directory, mode=0o700, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix=".status-", suffix=".json", dir=directory)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(data, handle)
        os.replace(tmp, path)
    except BaseException:
        try:
            os.unlink(tmp)
        except OSError:
            pass
        raise


def default_status_file() -> str:
    runtime = os.environ.get("XDG_RUNTIME_DIR") or "/run/user/%d" % os.getuid()
    return os.path.join(runtime, "audiobridge", "status.json")


def main(argv: Optional[List[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="audiobridge-sender",
                                     description="Stream system audio to a VBAN receiver.")
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", type=int, default=vban.DEFAULT_PORT)
    parser.add_argument("--preset", choices=sorted(PRESETS), default="balanced")
    parser.add_argument("--stream-name", default="AudioBridge")
    parser.add_argument("--status-file", default=None)
    parser.add_argument("--source", choices=("system", "tone"), default="system")
    parser.add_argument("--duration", type=float, default=0.0,
                        help="stop after this many seconds (0 runs until stopped)")
    args = parser.parse_args(argv)
    logging.basicConfig(level=logging.INFO, stream=sys.stderr, format="%(levelname)s %(message)s")
    engine = Engine(args.host, args.port, args.preset, args.stream_name,
                    args.status_file or default_status_file(), args.source, args.duration)
    return engine.run()
