import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "py_modules"))

from audiobridge import vban  # noqa: E402


class HeaderTests(unittest.TestCase):
    def test_audio_header_layout(self):
        header = vban.audio_header(48000, 128, 2, vban.DATATYPE_INT16, "AudioBridge", 7)
        self.assertEqual(len(header), 28)
        self.assertEqual(header[:4], b"VBAN")
        self.assertEqual(header[4], 3)            # 48 kHz index, audio protocol
        self.assertEqual(header[5], 127)          # samples - 1
        self.assertEqual(header[6], 1)            # channels - 1
        self.assertEqual(header[7], 1)            # INT16, PCM codec
        self.assertEqual(header[8:19], b"AudioBridge")
        self.assertEqual(header[19:24], b"\0" * 5)
        self.assertEqual(header[24:], (7).to_bytes(4, "little"))

    def test_roundtrip(self):
        header = vban.audio_header(44100, 256, 1, vban.DATATYPE_INT24, "x" * 30, 0xFFFFFFFF)
        parsed = vban.parse_header(header)
        self.assertEqual(parsed.protocol, vban.PROTOCOL_AUDIO)
        self.assertEqual(parsed.rate, 44100)
        self.assertEqual(parsed.nbs, 255)
        self.assertEqual(parsed.nbc, 0)
        self.assertEqual(parsed.bits & vban.DATATYPE_MASK, vban.DATATYPE_INT24)
        self.assertEqual(parsed.stream_name, "x" * 15)
        self.assertEqual(parsed.frame, 0xFFFFFFFF)

    def test_rejects_bad_values(self):
        with self.assertRaises(vban.VbanError):
            vban.audio_header(47999, 128, 2, vban.DATATYPE_INT16, "a", 0)
        with self.assertRaises(vban.VbanError):
            vban.audio_header(48000, 257, 2, vban.DATATYPE_INT16, "a", 0)
        with self.assertRaises(vban.VbanError):
            vban.parse_header(b"VBAX" + b"\0" * 24)
        with self.assertRaises(vban.VbanError):
            vban.parse_header(b"VBAN")

    def test_max_samples_fits_data_limit(self):
        self.assertEqual(vban.max_samples(2, vban.DATATYPE_INT16), 256)
        self.assertEqual(vban.max_samples(2, vban.DATATYPE_INT24), 239)
        self.assertLessEqual(vban.max_samples(8, vban.DATATYPE_INT24) * 8 * 3, vban.MAX_DATA_SIZE)


class PingTests(unittest.TestCase):
    def test_ping_request_and_reply(self):
        identity = vban.Identity(device_type=vban.TYPE_RECEPTOR, application="Audio Bridge Receiver",
                                 host_name="WORK-PC", device_name="Speakers", comment="audiobridge=1;buffer=40")
        packet = vban.ping_packet(identity, 42, reply=True)
        self.assertEqual(len(packet), 28 + 676)
        self.assertEqual(packet[4], 0x60)
        self.assertEqual(packet[5], 0x80)
        self.assertEqual(packet[6], 0)
        self.assertEqual(packet[7], 0)
        is_reply, frame, parsed = vban.parse_ping(packet)
        self.assertTrue(is_reply)
        self.assertEqual(frame, 42)
        self.assertEqual(parsed.host_name, "WORK-PC")
        self.assertEqual(parsed.application, "Audio Bridge Receiver")
        self.assertEqual(parsed.device_type, vban.TYPE_RECEPTOR)
        self.assertEqual(vban.parse_comment(parsed.comment), {"audiobridge": "1", "buffer": "40"})

    def test_field_offsets(self):
        packet = vban.ping_packet(vban.Identity(host_name="H", application="A", device_name="D"), 1)
        payload = packet[28:]
        self.assertEqual(payload[164:165], b"D")   # DeviceName_ascii
        self.assertEqual(payload[292:293], b"A")   # ApplicationName_ascii
        self.assertEqual(payload[356:357], b"H")   # HostName_ascii

    def test_parse_ping_ignores_other_packets(self):
        self.assertIsNone(vban.parse_ping(b"junk"))
        audio = vban.audio_header(48000, 1, 2, vban.DATATYPE_INT16, "a", 0) + b"\0" * 4
        self.assertIsNone(vban.parse_ping(audio))
        short = vban.ping_packet(vban.Identity(), 1)[:100]
        self.assertIsNone(vban.parse_ping(short))

    def test_long_text_is_truncated_and_terminated(self):
        packet = vban.ping_packet(vban.Identity(host_name="h" * 200, comment="ż" * 200), 1)
        self.assertEqual(len(packet), 704)
        _, _, parsed = vban.parse_ping(packet)
        self.assertEqual(parsed.host_name, "h" * 63)


if __name__ == "__main__":
    unittest.main()
