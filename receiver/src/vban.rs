//! VBAN packet decoding (PCM audio) and the PING0 identification reply.
//! Layout follows the VB-Audio VBAN protocol specification, revision 13.

pub const DEFAULT_PORT: u16 = 6980;
pub const HEADER_SIZE: usize = 28;
pub const PING0_SIZE: usize = 676;

pub const PROTOCOL_MASK: u8 = 0xE0;
pub const PROTOCOL_AUDIO: u8 = 0x00;
pub const PROTOCOL_SERVICE: u8 = 0x60;
const SR_MASK: u8 = 0x1F;
const DATATYPE_MASK: u8 = 0x07;
const CODEC_MASK: u8 = 0xF0;
const CODEC_PCM: u8 = 0x00;

pub const SERVICE_IDENTIFICATION: u8 = 0;
pub const FUNCTION_PING0: u8 = 0x00;
pub const FUNCTION_REPLY: u8 = 0x80;

pub const TYPE_RECEPTOR: u32 = 0x0000_0001;
pub const FEATURE_AUDIO: u32 = 0x0000_0001;

pub const SAMPLE_RATES: [u32; 21] = [
    6000, 12000, 24000, 48000, 96000, 192000, 384000, 8000, 16000, 32000, 64000, 128000, 256000,
    512000, 11025, 22050, 44100, 88200, 176400, 352800, 705600,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    U8,
    I16,
    I24,
    I32,
    F32,
}

impl SampleFormat {
    fn from_bits(bits: u8) -> Option<Self> {
        match bits & DATATYPE_MASK {
            0 => Some(Self::U8),
            1 => Some(Self::I16),
            2 => Some(Self::I24),
            3 => Some(Self::I32),
            4 => Some(Self::F32),
            _ => None,
        }
    }

    pub fn bytes(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::I16 => 2,
            Self::I24 => 3,
            Self::I32 | Self::F32 => 4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::U8 => "8-bit",
            Self::I16 => "16-bit",
            Self::I24 => "24-bit",
            Self::I32 => "32-bit",
            Self::F32 => "32-bit float",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub protocol: u8,
    pub sr_index: u8,
    pub nbs: u8,
    pub nbc: u8,
    pub bits: u8,
    pub stream_name: String,
    pub frame: u32,
}

pub fn parse_header(packet: &[u8]) -> Option<Header> {
    if packet.len() < HEADER_SIZE || &packet[..4] != b"VBAN" {
        return None;
    }
    let name = &packet[8..24];
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    Some(Header {
        protocol: packet[4] & PROTOCOL_MASK,
        sr_index: packet[4] & SR_MASK,
        nbs: packet[5],
        nbc: packet[6],
        bits: packet[7],
        stream_name: String::from_utf8_lossy(&name[..end]).into_owned(),
        frame: u32::from_le_bytes([packet[24], packet[25], packet[26], packet[27]]),
    })
}

/// A decoded audio packet: stereo frames in -1..1.
#[derive(Debug, Clone)]
pub struct AudioPacket {
    pub rate: u32,
    pub channels: usize,
    pub format: SampleFormat,
    pub frame: u32,
    pub stream_name: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AudioError {
    NotAudio,
    UnsupportedCodec,
    UnsupportedFormat,
    UnknownRate,
    WrongSize,
}

/// Validate an audio packet and append its frames (as stereo) to `out`.
pub fn decode_audio(packet: &[u8], out: &mut Vec<[f32; 2]>) -> Result<AudioPacket, AudioError> {
    let header = parse_header(packet).ok_or(AudioError::NotAudio)?;
    if header.protocol != PROTOCOL_AUDIO {
        return Err(AudioError::NotAudio);
    }
    if header.bits & CODEC_MASK != CODEC_PCM || header.bits & 0x08 != 0 {
        return Err(AudioError::UnsupportedCodec);
    }
    let format = SampleFormat::from_bits(header.bits).ok_or(AudioError::UnsupportedFormat)?;
    let rate = *SAMPLE_RATES
        .get(header.sr_index as usize)
        .ok_or(AudioError::UnknownRate)?;
    let channels = header.nbc as usize + 1;
    let samples = header.nbs as usize + 1;
    let size = format.bytes();
    let data = &packet[HEADER_SIZE..];
    if data.len() != samples * channels * size {
        return Err(AudioError::WrongSize);
    }
    out.reserve(samples);
    let stride = channels * size;
    for i in 0..samples {
        let frame = &data[i * stride..(i + 1) * stride];
        let left = sample(&frame[..size], format);
        let right = if channels > 1 {
            sample(&frame[size..2 * size], format)
        } else {
            left
        };
        out.push([left, right]);
    }
    Ok(AudioPacket {
        rate,
        channels,
        format,
        frame: header.frame,
        stream_name: header.stream_name,
    })
}

fn sample(b: &[u8], format: SampleFormat) -> f32 {
    match format {
        SampleFormat::U8 => (b[0] as f32 - 128.0) / 128.0,
        SampleFormat::I16 => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
        SampleFormat::I24 => {
            let v = i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8;
            v as f32 / 8_388_608.0
        }
        SampleFormat::I32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
        SampleFormat::F32 => {
            let v = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            if v.is_finite() {
                v.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        }
    }
}

/// A PING0 request we may answer. `comment` carries Audio Bridge hints.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PingRequest {
    pub frame: u32,
    pub host_name: String,
    pub application: String,
    pub comment: String,
}

fn text_at(payload: &[u8], offset: usize, len: usize) -> String {
    let field = &payload[offset..offset + len];
    let end = field.iter().position(|&b| b == 0).unwrap_or(len);
    String::from_utf8_lossy(&field[..end]).into_owned()
}

pub fn parse_ping_request(packet: &[u8]) -> Option<PingRequest> {
    let header = parse_header(packet)?;
    if header.protocol != PROTOCOL_SERVICE || header.nbc != SERVICE_IDENTIFICATION {
        return None;
    }
    if header.nbs != FUNCTION_PING0 {
        return None; // a reply, or another function
    }
    let payload = &packet[HEADER_SIZE..];
    if payload.len() < PING0_SIZE {
        return None;
    }
    Some(PingRequest {
        frame: header.frame,
        application: text_at(payload, 292, 64),
        host_name: text_at(payload, 356, 64),
        comment: text_at(payload, 548, 128),
    })
}

pub struct Identity<'a> {
    pub preferred_rate: u32,
    pub device_name: &'a str,
    pub host_name: &'a str,
    pub version: [u8; 4],
}

fn put_text(buf: &mut [u8], offset: usize, len: usize, text: &str) {
    let bytes = text.as_bytes();
    let mut n = bytes.len().min(len - 1);
    while n > 0 && !text.is_char_boundary(n) {
        n -= 1;
    }
    buf[offset..offset + n].copy_from_slice(&bytes[..n]);
}

/// The `_ascii` fields of PING0 are plain ASCII. Device and computer names
/// often are not ("Słuchawki"), so fold common accented letters and replace
/// anything else.
pub fn to_ascii(text: &str) -> String {
    const FROM: &str =
        "ąćęłńóśźżĄĆĘŁŃÓŚŹŻáàâäãåčďéèêëěíìîïňòôöõřšťúùûüůýÿžçñÁÀÂÄÃÅČĎÉÈÊËĚÍÌÎÏŇÒÔÖÕŘŠŤÚÙÛÜŮÝŽÇÑ";
    const TO: &str =
        "acelnoszzACELNOSZZaaaaaacdeeeeeiiiinoooorstuuuuuyyzcnAAAAAACDEEEEEIIIINOOOORSTUUUUUYZCN";
    text.chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else if let Some(i) = FROM.chars().position(|f| f == c) {
                TO.as_bytes()[i] as char
            } else if c == 'ß' {
                's'
            } else {
                '?'
            }
        })
        .collect()
}

pub fn ping_reply(request_frame: u32, id: &Identity) -> Vec<u8> {
    let mut packet = vec![0u8; HEADER_SIZE + PING0_SIZE];
    packet[..4].copy_from_slice(b"VBAN");
    packet[4] = PROTOCOL_SERVICE;
    packet[5] = FUNCTION_PING0 | FUNCTION_REPLY;
    packet[6] = SERVICE_IDENTIFICATION;
    packet[7] = 0;
    packet[8..13].copy_from_slice(b"PING0");
    packet[24..28].copy_from_slice(&request_frame.to_le_bytes());
    let p = &mut packet[HEADER_SIZE..];
    p[0..4].copy_from_slice(&TYPE_RECEPTOR.to_le_bytes());
    p[4..8].copy_from_slice(&FEATURE_AUDIO.to_le_bytes());
    p[12..16].copy_from_slice(&id.preferred_rate.to_le_bytes());
    p[16..20].copy_from_slice(&8000u32.to_le_bytes());
    p[20..24].copy_from_slice(&192000u32.to_le_bytes());
    p[28..32].copy_from_slice(&id.version);
    put_text(p, 48, 8, "EN");
    put_text(p, 164, 64, &to_ascii(id.device_name));
    put_text(p, 228, 64, "60plus");
    put_text(p, 292, 64, "Audio Bridge Receiver");
    put_text(p, 356, 64, &to_ascii(id.host_name));
    put_text(p, 548, 128, "audiobridge=1");
    packet
}

/// Read `key=value;key=value` hints.
pub fn comment_value<'a>(comment: &'a str, key: &str) -> Option<&'a str> {
    comment.split(';').find_map(|part| {
        let (k, v) = part.split_once('=')?;
        (k.trim() == key).then(|| v.trim())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sr: u8, nbs: u8, nbc: u8, bits: u8, frame: u32) -> Vec<u8> {
        let mut p = vec![0u8; HEADER_SIZE];
        p[..4].copy_from_slice(b"VBAN");
        p[4] = sr;
        p[5] = nbs;
        p[6] = nbc;
        p[7] = bits;
        p[8..12].copy_from_slice(b"Test");
        p[24..28].copy_from_slice(&frame.to_le_bytes());
        p
    }

    #[test]
    fn decodes_int16_stereo() {
        let mut p = header(3, 1, 1, 1, 9);
        for v in [16384i16, -16384, 32767, -32768] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        let mut out = Vec::new();
        let info = decode_audio(&p, &mut out).unwrap();
        assert_eq!(info.rate, 48000);
        assert_eq!(info.frame, 9);
        assert_eq!(info.stream_name, "Test");
        assert_eq!(out.len(), 2);
        assert!((out[0][0] - 0.5).abs() < 1e-4 && (out[0][1] + 0.5).abs() < 1e-4);
        assert!((out[1][1] + 1.0).abs() < 1e-6);
    }

    #[test]
    fn decodes_int24_mono_as_stereo() {
        let mut p = header(16, 0, 0, 2, 0);
        p.extend_from_slice(&[0x00, 0x00, 0xC0]); // -0.5
        let mut out = Vec::new();
        let info = decode_audio(&p, &mut out).unwrap();
        assert_eq!(info.rate, 44100);
        assert!((out[0][0] + 0.5).abs() < 1e-6);
        assert_eq!(out[0][0], out[0][1]);
    }

    #[test]
    fn takes_first_two_of_many_channels() {
        let mut p = header(3, 0, 5, 4, 0);
        for v in [0.25f32, -0.25, 1.0, 1.0, 1.0, 1.0] {
            p.extend_from_slice(&v.to_le_bytes());
        }
        let mut out = Vec::new();
        decode_audio(&p, &mut out).unwrap();
        assert_eq!(out, vec![[0.25, -0.25]]);
    }

    #[test]
    fn rejects_bad_packets() {
        let mut out = Vec::new();
        assert_eq!(
            decode_audio(b"VBAX", &mut out).unwrap_err(),
            AudioError::NotAudio
        );
        let short = header(3, 1, 1, 1, 0);
        assert_eq!(
            decode_audio(&short, &mut out).unwrap_err(),
            AudioError::WrongSize
        );
        let mut codec = header(3, 0, 0, 0x11, 0);
        codec.extend_from_slice(&[0, 0]);
        assert_eq!(
            decode_audio(&codec, &mut out).unwrap_err(),
            AudioError::UnsupportedCodec
        );
        let mut rate = header(25, 0, 0, 1, 0);
        rate.extend_from_slice(&[0, 0]);
        assert_eq!(
            decode_audio(&rate, &mut out).unwrap_err(),
            AudioError::UnknownRate
        );
        let mut service = header(0x60, 0, 0, 0, 0);
        service.extend_from_slice(&[0; 676]);
        assert_eq!(
            decode_audio(&service, &mut out).unwrap_err(),
            AudioError::NotAudio
        );
        assert!(out.is_empty());
    }

    #[test]
    fn ping_reply_layout() {
        let id = Identity {
            preferred_rate: 48000,
            device_name: "Speakers",
            host_name: "WORK-PC",
            version: [0, 1, 0, 0],
        };
        let reply = ping_reply(77, &id);
        assert_eq!(reply.len(), 704);
        assert_eq!(reply[4], 0x60);
        assert_eq!(reply[5], 0x80);
        assert_eq!(u32::from_le_bytes(reply[24..28].try_into().unwrap()), 77);
        let p = &reply[HEADER_SIZE..];
        assert_eq!(&p[164..172], b"Speakers");
        assert_eq!(&p[356..363], b"WORK-PC");
        assert_eq!(&p[292..313], b"Audio Bridge Receiver");
        // a reply is not a request
        assert!(parse_ping_request(&reply).is_none());
    }

    #[test]
    fn reads_ping_request() {
        let mut p = header(0x60, 0, 0, 0, 5);
        let mut payload = vec![0u8; PING0_SIZE];
        payload[292..304].copy_from_slice(b"Audio Bridge");
        payload[356..365].copy_from_slice(b"steamdeck");
        let c = b"audiobridge=1;preset=low;buffer=20";
        payload[548..548 + c.len()].copy_from_slice(c);
        p.extend_from_slice(&payload);
        let req = parse_ping_request(&p).unwrap();
        assert_eq!(req.frame, 5);
        assert_eq!(req.host_name, "steamdeck");
        assert_eq!(comment_value(&req.comment, "buffer"), Some("20"));
        assert_eq!(comment_value(&req.comment, "nope"), None);
    }

    #[test]
    fn ascii_fields_are_folded() {
        assert_eq!(to_ascii("Słuchawki (Px8 S2)"), "Sluchawki (Px8 S2)");
        assert_eq!(to_ascii("Głośniki Żółć"), "Glosniki Zolc");
        assert_eq!(to_ascii("音"), "?");
        let id = Identity {
            preferred_rate: 48000,
            device_name: "Słuchawki",
            host_name: "PC",
            version: [0; 4],
        };
        let reply = ping_reply(1, &id);
        assert_eq!(&reply[HEADER_SIZE + 164..HEADER_SIZE + 173], b"Sluchawki");
    }

    #[test]
    fn text_is_cut_on_char_boundary() {
        let mut buf = [0u8; 8];
        put_text(&mut buf, 0, 4, "żółw");
        assert_eq!(&buf[..2], "ż".as_bytes());
        assert_eq!(buf[2], 0);
    }
}
