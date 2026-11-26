"""A minimal VBAN receiver for tests: records audio packets, answers pings."""

import os
import socket
import sys
import threading

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

from audiobridge import vban  # noqa: E402


class FakeReceiver:
    def __init__(self, answer=True, name="TEST-PC"):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(("127.0.0.1", 0))
        self.sock.settimeout(0.1)
        self.port = self.sock.getsockname()[1]
        self.answer = answer
        self.identity = vban.Identity(device_type=vban.TYPE_RECEPTOR, application="Audio Bridge Receiver",
                                      host_name=name, device_name="Speakers")
        self.audio = []
        self.pings = []
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def __enter__(self):
        self._thread.start()
        return self

    def __exit__(self, *exc):
        self._stop.set()
        self._thread.join(2)
        self.sock.close()

    def _run(self):
        while not self._stop.is_set():
            try:
                data, addr = self.sock.recvfrom(4096)
            except socket.timeout:
                continue
            except OSError:
                return
            ping = vban.parse_ping(data)
            if ping:
                is_reply, frame, ident = ping
                if not is_reply:
                    self.pings.append((frame, ident))
                    if self.answer:
                        self.sock.sendto(vban.ping_packet(self.identity, frame, reply=True), addr)
                continue
            try:
                header = vban.parse_header(data)
            except vban.VbanError:
                continue
            if header.protocol == vban.PROTOCOL_AUDIO:
                self.audio.append((header, data[vban.HEADER_SIZE:]))
