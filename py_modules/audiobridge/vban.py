"""VBAN packet encoding and decoding.

Only the parts this project uses are implemented: PCM audio packets and the
PING0 identification service. Field layout follows the VB-Audio VBAN protocol
specification (revision 13). All integers are little endian.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from typing import Optional

DEFAULT_PORT = 6980
HEADER_SIZE = 28
MAX_DATA_SIZE = 1436
MAX_SAMPLES = 256
MAGIC = b"VBAN"

PROTOCOL_MASK = 0xE0
PROTOCOL_AUDIO = 0x00
PROTOCOL_SERVICE = 0x60
SR_MASK = 0x1F
DATATYPE_MASK = 0x07
CODEC_MASK = 0xF0
CODEC_PCM = 0x00

DATATYPE_INT16 = 0x01
DATATYPE_INT24 = 0x02
DATATYPE_INT32 = 0x03
DATATYPE_FLOAT32 = 0x04
SAMPLE_SIZE = {0: 1, 1: 2, 2: 3, 3: 4, 4: 4, 5: 8}

SAMPLE_RATES = (
    6000, 12000, 24000, 48000, 96000, 192000, 384000,
    8000, 16000, 32000, 64000, 128000, 256000, 512000,
    11025, 22050, 44100, 88200, 176400, 352800, 705600,
)

SERVICE_IDENTIFICATION = 0
FUNCTION_PING0 = 0x00
FUNCTION_REPLY = 0x80

TYPE_RECEPTOR = 0x00000001
TYPE_TRANSMITTER = 0x00000002
TYPE_RECEPTOR_SPOT = 0x00000004
TYPE_VIRTUAL_MIXER = 0x00000020
TYPE_CAN_RECEIVE = TYPE_RECEPTOR | TYPE_RECEPTOR_SPOT | TYPE_VIRTUAL_MIXER
FEATURE_AUDIO = 0x00000001

STREAM_NAME_SIZE = 16
PING0_SIZE = 676
_HEADER = struct.Struct("<4sBBBB16sI")
_PING0 = struct.Struct("<7I4s8s8s8s8s64s32sHH64s64s64s64s128s128s")
assert _HEADER.size == HEADER_SIZE
assert _PING0.size == PING0_SIZE


class VbanError(ValueError):
    """Raised for packets that are not valid VBAN."""


def rate_index(rate: int) -> int:
    try:
        return SAMPLE_RATES.index(rate)
    except ValueError:
        raise VbanError(f"Sample rate {rate} Hz is not supported by VBAN") from None


def _fixed(text: str, size: int, encoding: str = "ascii") -> bytes:
    raw = text.encode(encoding, "replace")[: size - 1]
    return raw + b"\0" * (size - len(raw))


def _text(raw: bytes, encoding: str = "utf-8") -> str:
    # The spec says ASCII, but some programs put UTF-8 in these fields.
    return raw.split(b"\0", 1)[0].decode(encoding, "replace")


def max_samples(channels: int, datatype: int) -> int:
    """Largest sample count per packet that fits the VBAN data limit."""
    frame = SAMPLE_SIZE[datatype] * channels
    return max(1, min(MAX_SAMPLES, MAX_DATA_SIZE // frame))


def audio_header(rate: int, samples: int, channels: int, datatype: int,
                 stream_name: str, frame: int) -> bytes:
    if not 1 <= samples <= MAX_SAMPLES:
        raise VbanError("samples per packet must be 1 to 256")
    if not 1 <= channels <= 256:
        raise VbanError("channel count must be 1 to 256")
    return _HEADER.pack(
        MAGIC,
        PROTOCOL_AUDIO | rate_index(rate),
        samples - 1,
        channels - 1,
        (datatype & DATATYPE_MASK) | CODEC_PCM,
        _fixed(stream_name, STREAM_NAME_SIZE),
        frame & 0xFFFFFFFF,
    )


@dataclass
class Header:
    protocol: int
    sr_index: int
    nbs: int
    nbc: int
    bits: int
    stream_name: str
    frame: int

    @property
    def rate(self) -> Optional[int]:
        return SAMPLE_RATES[self.sr_index] if self.sr_index < len(SAMPLE_RATES) else None


def parse_header(packet: bytes) -> Header:
    if len(packet) < HEADER_SIZE or packet[:4] != MAGIC:
        raise VbanError("not a VBAN packet")
    _, sr, nbs, nbc, bits, name, frame = _HEADER.unpack_from(packet)
    return Header(sr & PROTOCOL_MASK, sr & SR_MASK, nbs, nbc, bits, _text(name), frame)


@dataclass
class Identity:
    """The fields of a PING0 payload this project reads or writes."""

    device_type: int = TYPE_TRANSMITTER
    features: int = FEATURE_AUDIO
    preferred_rate: int = 48000
    min_rate: int = 48000
    max_rate: int = 48000
    version: tuple = (0, 1, 0, 0)
    device_name: str = ""
    manufacturer: str = ""
    application: str = ""
    host_name: str = ""
    user_name: str = ""
    comment: str = ""

    def pack(self) -> bytes:
        return _PING0.pack(
            self.device_type, self.features, 0,
            self.preferred_rate, self.min_rate, self.max_rate, 0,
            bytes(int(v) & 0xFF for v in (tuple(self.version) + (0, 0, 0, 0))[:4]),
            b"", b"", _fixed("EN", 8), b"", b"", b"", 0, 0,
            _fixed(self.device_name, 64), _fixed(self.manufacturer, 64),
            _fixed(self.application, 64), _fixed(self.host_name, 64),
            _fixed(self.user_name, 128, "utf-8"), _fixed(self.comment, 128, "utf-8"),
        )

    @classmethod
    def unpack(cls, payload: bytes) -> "Identity":
        if len(payload) < PING0_SIZE:
            raise VbanError("PING0 payload is too short")
        f = _PING0.unpack_from(payload)
        return cls(
            device_type=f[0], features=f[1], preferred_rate=f[3], min_rate=f[4], max_rate=f[5],
            version=tuple(f[7]), device_name=_text(f[16]), manufacturer=_text(f[17]),
            application=_text(f[18]), host_name=_text(f[19]),
            user_name=_text(f[20], "utf-8"), comment=_text(f[21], "utf-8"),
        )


def ping_packet(identity: Identity, frame: int, reply: bool = False) -> bytes:
    header = _HEADER.pack(
        MAGIC, PROTOCOL_SERVICE,
        FUNCTION_REPLY if reply else FUNCTION_PING0,
        SERVICE_IDENTIFICATION, 0,
        _fixed("PING0", STREAM_NAME_SIZE), frame & 0xFFFFFFFF,
    )
    return header + identity.pack()


def parse_ping(packet: bytes):
    """Return (is_reply, frame, Identity) for a PING0 packet, or None."""
    try:
        header = parse_header(packet)
    except VbanError:
        return None
    if header.protocol != PROTOCOL_SERVICE or header.nbc != SERVICE_IDENTIFICATION:
        return None
    if header.nbs & 0x7F != FUNCTION_PING0:
        return None
    try:
        identity = Identity.unpack(packet[HEADER_SIZE:])
    except VbanError:
        return None
    return bool(header.nbs & FUNCTION_REPLY), header.frame, identity


def parse_comment(comment: str) -> dict:
    """Read the `key=value;key=value` hints Audio Bridge puts in UserComment."""
    hints = {}
    for part in comment.split(";"):
        key, sep, value = part.partition("=")
        if sep and key.strip():
            hints[key.strip()] = value.strip()
    return hints
