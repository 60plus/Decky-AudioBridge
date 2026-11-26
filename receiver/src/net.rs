//! The network side: one thread that receives VBAN packets, feeds the
//! playback buffer and answers PING0 requests.

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, SockAddr, Socket, Type};

use crate::log;
use crate::netinfo;
use crate::playout::Playout;
use crate::settings::Delay;
use crate::vban::{self, SampleFormat};

/// After this long without audio the sender is considered gone and another
/// one may take over.
pub const SENDER_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_REPLIES_PER_SECOND: u32 = 50;

#[derive(Debug, Clone)]
pub struct Sender {
    pub ip: Ipv4Addr,
    pub port: u16,
    pub stream: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct NetStatus {
    pub listening_on: Option<SocketAddrV4>,
    pub bind_error: Option<String>,
    pub sender: Option<Sender>,
    pub last_sender: Option<Sender>,
    pub last_audio: Option<Instant>,
    pub last_ping: Option<(Instant, String, Ipv4Addr)>,
    pub format: Option<(u32, usize, SampleFormat)>,
    pub packets: u64,
    pub rejected: u64,
    pub ignored_other: u64,
    pub hint_buffer_ms: Option<f64>,
}

impl NetStatus {
    pub fn playing(&self) -> bool {
        self.sender.is_some()
            && self
                .last_audio
                .map(|t| t.elapsed() < SENDER_TIMEOUT)
                .unwrap_or(false)
    }
}

pub struct Shared {
    pub playout: Arc<Mutex<Playout>>,
    pub status: Mutex<NetStatus>,
    pub delay: Mutex<Delay>,
    pub answer_discovery: AtomicBool,
    pub device_name: Mutex<String>,
    pub output_rate: Mutex<u32>,
}

pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Shared {
    pub fn new(playout: Arc<Mutex<Playout>>, delay: Delay, answer_discovery: bool) -> Self {
        Self {
            playout,
            status: Mutex::new(NetStatus::default()),
            delay: Mutex::new(delay),
            answer_discovery: AtomicBool::new(answer_discovery),
            device_name: Mutex::new(String::new()),
            output_rate: Mutex::new(48000),
        }
    }

    /// Apply the delay setting together with the sender's hint.
    pub fn apply_delay(&self) {
        let delay = *lock(&self.delay);
        let hint = lock(&self.status).hint_buffer_ms;
        lock(&self.playout).set_target(delay.target_ms(hint), delay == Delay::Auto);
    }
}

pub struct Listener {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Listener {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Where to bind. `listen` is "auto", "all" or an IPv4 address.
pub fn bind_ip(listen: &str) -> Ipv4Addr {
    match listen {
        "all" => Ipv4Addr::UNSPECIFIED,
        "auto" => netinfo::main_ip().unwrap_or(Ipv4Addr::UNSPECIFIED),
        other => other.parse().unwrap_or(Ipv4Addr::UNSPECIFIED),
    }
}

fn open_socket(ip: Ipv4Addr, port: u16) -> std::io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    let _ = socket.set_recv_buffer_size(1 << 20);
    socket.bind(&SockAddr::from(SocketAddrV4::new(ip, port)))?;
    let socket: UdpSocket = socket.into();
    socket.set_read_timeout(Some(Duration::from_millis(200)))?;
    Ok(socket)
}

fn bind_message(err: &std::io::Error, ip: Ipv4Addr, port: u16) -> String {
    match err.kind() {
        ErrorKind::AddrInUse => format!(
            "Port {port} is already used by another program. Close the other copy of Audio Bridge Receiver \
             or any other VBAN program (such as Voicemeeter), or choose another port."
        ),
        ErrorKind::AddrNotAvailable => format!("This PC no longer has the address {ip}. Choose another network in Settings."),
        ErrorKind::PermissionDenied => format!("Windows did not allow listening on port {port}."),
        _ => format!("Cannot listen on {ip}:{port}: {err}"),
    }
}

pub fn start(shared: Arc<Shared>, listen: String, port: u16) -> Listener {
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = std::thread::Builder::new()
        .name("network".into())
        .spawn(move || run(shared, listen, port, thread_stop))
        .ok();
    Listener { stop, thread }
}

fn run(shared: Arc<Shared>, listen: String, port: u16, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::SeqCst) {
        let ip = bind_ip(&listen);
        let socket = match open_socket(ip, port) {
            Ok(s) => s,
            Err(err) => {
                let message = bind_message(&err, ip, port);
                log!("{message}");
                {
                    let mut st = lock(&shared.status);
                    st.bind_error = Some(message);
                    st.listening_on = None;
                }
                // Try again now and then: the port may free up, or the network may come back.
                for _ in 0..25 {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                continue;
            }
        };
        log!("listening on {ip}:{port}");
        {
            let mut st = lock(&shared.status);
            st.bind_error = None;
            st.listening_on = Some(SocketAddrV4::new(ip, port));
        }
        let rebind = serve(&shared, &socket, &listen, ip, &stop);
        drop(socket);
        if !rebind {
            break;
        }
        log!("network address changed, listening again");
    }
    let mut st = lock(&shared.status);
    st.listening_on = None;
    st.sender = None;
}

/// Returns true when the socket should be reopened on a new address.
fn serve(
    shared: &Shared,
    socket: &UdpSocket,
    listen: &str,
    bound: Ipv4Addr,
    stop: &AtomicBool,
) -> bool {
    let mut buf = [0u8; 2048];
    let mut frames: Vec<[f32; 2]> = Vec::with_capacity(512);
    let mut replies = (Instant::now(), 0u32);
    let mut last_addr_check = Instant::now();
    let host_name = netinfo::computer_name();
    let version = {
        let mut v = [0u8; 4];
        for (i, part) in env!("CARGO_PKG_VERSION").split('.').take(3).enumerate() {
            v[i] = part.parse().unwrap_or(0);
        }
        v
    };

    while !stop.load(Ordering::SeqCst) {
        if listen == "auto" && last_addr_check.elapsed() > Duration::from_secs(5) {
            last_addr_check = Instant::now();
            if let Some(ip) = netinfo::main_ip() {
                if ip != bound {
                    return true;
                }
            }
        }
        let (n, src) = match socket.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(src))) => (n, src),
            Ok(_) => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                check_timeout(shared);
                continue;
            }
            Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
            Err(e) => {
                log!("receive error: {e}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let packet = &buf[..n];
        if n < vban::HEADER_SIZE || &packet[..4] != b"VBAN" || !netinfo::is_local_sender(*src.ip())
        {
            lock(&shared.status).rejected += 1;
            continue;
        }
        match packet[4] & vban::PROTOCOL_MASK {
            vban::PROTOCOL_SERVICE => {
                if let Some(request) = vban::parse_ping_request(packet) {
                    handle_ping(
                        shared,
                        socket,
                        src,
                        &request,
                        &host_name,
                        version,
                        &mut replies,
                    );
                }
            }
            vban::PROTOCOL_AUDIO => {
                frames.clear();
                match vban::decode_audio(packet, &mut frames) {
                    Ok(info) => handle_audio(shared, src, &info, &frames),
                    Err(_) => lock(&shared.status).rejected += 1,
                }
            }
            _ => {}
        }
        check_timeout(shared);
    }
    false
}

fn check_timeout(shared: &Shared) {
    let mut st = lock(&shared.status);
    if st.sender.is_some()
        && st
            .last_audio
            .map(|t| t.elapsed() > SENDER_TIMEOUT)
            .unwrap_or(true)
    {
        if let Some(sender) = st.sender.take() {
            log!("sender {} stopped sending", sender.ip);
            st.last_sender = Some(sender);
        }
        drop(st);
        lock(&shared.playout).sender_gone();
    }
}

fn handle_ping(
    shared: &Shared,
    socket: &UdpSocket,
    src: SocketAddrV4,
    request: &vban::PingRequest,
    host_name: &str,
    version: [u8; 4],
    replies: &mut (Instant, u32),
) {
    // Anyone on the network can send a ping, so only accept a sane number.
    let hint = vban::comment_value(&request.comment, "buffer")
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite() && (5.0..=500.0).contains(v));
    let ours = vban::comment_value(&request.comment, "audiobridge") == Some("1");
    let (is_current, idle) = {
        let mut st = lock(&shared.status);
        let current = st
            .sender
            .as_ref()
            .map(|s| s.ip == *src.ip())
            .unwrap_or(false);
        let idle = st.sender.is_none();
        if current || (idle && ours && hint.is_some()) {
            let name = if request.host_name.is_empty() {
                src.ip().to_string()
            } else {
                request.host_name.clone()
            };
            st.last_ping = Some((Instant::now(), name.clone(), *src.ip()));
            if let Some(sender) = st.sender.as_mut() {
                sender.name = Some(name);
            }
        }
        (current, idle)
    };
    // A streaming plugin sends its suggested buffer with every ping.
    if let Some(ms) = hint {
        if is_current || idle {
            let changed = {
                let mut st = lock(&shared.status);
                let changed = st.hint_buffer_ms != Some(ms);
                st.hint_buffer_ms = Some(ms);
                changed
            };
            if changed {
                shared.apply_delay();
            }
        }
    }
    if !is_current && !shared.answer_discovery.load(Ordering::Relaxed) {
        return;
    }
    if replies.0.elapsed() > Duration::from_secs(1) {
        *replies = (Instant::now(), 0);
    }
    if replies.1 >= MAX_REPLIES_PER_SECOND {
        return;
    }
    replies.1 += 1;
    let device = lock(&shared.device_name).clone();
    let rate = *lock(&shared.output_rate);
    let reply = vban::ping_reply(
        request.frame,
        &vban::Identity {
            preferred_rate: rate,
            device_name: &device,
            host_name,
            version,
        },
    );
    let _ = socket.send_to(&reply, SocketAddr::V4(src));
}

fn handle_audio(shared: &Shared, src: SocketAddrV4, info: &vban::AudioPacket, frames: &[[f32; 2]]) {
    let now = Instant::now();
    let mut new_stream = false;
    {
        let mut st = lock(&shared.status);
        let accept = match &st.sender {
            None => true,
            Some(s) if s.ip == *src.ip() && s.stream == info.stream_name => {
                if s.port != src.port() {
                    // Same machine, new sender process: start clean.
                    new_stream = true;
                }
                true
            }
            Some(_) => {
                st.ignored_other += 1;
                false
            }
        };
        if !accept {
            return;
        }
        if st.sender.is_none() || new_stream {
            let name = st
                .last_ping
                .as_ref()
                .filter(|(_, _, ip)| ip == src.ip())
                .map(|(_, name, _)| name.clone());
            log!(
                "receiving \"{}\" from {} ({} Hz, {} ch, {})",
                info.stream_name,
                src,
                info.rate,
                info.channels,
                info.format.label()
            );
            st.sender = Some(Sender {
                ip: *src.ip(),
                port: src.port(),
                stream: info.stream_name.clone(),
                name,
            });
            new_stream = true;
        }
        st.last_audio = Some(now);
        st.format = Some((info.rate, info.channels, info.format));
        st.packets += 1;
    }
    let mut playout = lock(&shared.playout);
    if new_stream {
        playout.new_stream();
    }
    playout.push(frames, info.frame, info.rate);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio_packet(frame: u32, stream: &str) -> Vec<u8> {
        let mut p = vec![0u8; vban::HEADER_SIZE];
        p[..4].copy_from_slice(b"VBAN");
        p[4] = 3;
        p[5] = 127;
        p[6] = 1;
        p[7] = 1;
        p[8..8 + stream.len()].copy_from_slice(stream.as_bytes());
        p[24..28].copy_from_slice(&frame.to_le_bytes());
        p.extend(std::iter::repeat(0u8).take(128 * 4));
        p
    }

    fn ping(frame: u32, comment: &str) -> Vec<u8> {
        let mut p = vec![0u8; vban::HEADER_SIZE + vban::PING0_SIZE];
        p[..4].copy_from_slice(b"VBAN");
        p[4] = 0x60;
        p[24..28].copy_from_slice(&frame.to_le_bytes());
        p[28 + 356..28 + 365].copy_from_slice(b"steamdeck");
        p[28 + 548..28 + 548 + comment.len()].copy_from_slice(comment.as_bytes());
        p
    }

    #[test]
    fn end_to_end_over_loopback() {
        let playout = Arc::new(Mutex::new(Playout::new(40.0, true)));
        let shared = Arc::new(Shared::new(playout.clone(), Delay::Auto, true));
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let listener = start(shared.clone(), "127.0.0.1".into(), port);
        std::thread::sleep(Duration::from_millis(150));

        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let target = format!("127.0.0.1:{port}");
        sender
            .send_to(&ping(9, "audiobridge=1;preset=low;buffer=20"), &target)
            .unwrap();
        let mut buf = [0u8; 1024];
        let (n, _) = sender.recv_from(&mut buf).expect("ping reply");
        assert_eq!(n, 704);
        assert_eq!(buf[5], 0x80);
        assert_eq!(u32::from_le_bytes(buf[24..28].try_into().unwrap()), 9);
        assert_eq!(&buf[28 + 292..28 + 313], b"Audio Bridge Receiver");

        for frame in 0..30u32 {
            sender
                .send_to(&audio_packet(frame, "AudioBridge"), &target)
                .unwrap();
        }
        // a second sender is ignored while the first is active
        let other = UdpSocket::bind("127.0.0.1:0").unwrap();
        other.send_to(&audio_packet(0, "Other"), &target).unwrap();
        // junk is rejected
        sender.send_to(b"hello", &target).unwrap();
        std::thread::sleep(Duration::from_millis(200));

        {
            let st = lock(&shared.status);
            assert_eq!(st.packets, 30);
            assert_eq!(st.ignored_other, 1);
            assert_eq!(st.rejected, 1);
            assert!(st.playing());
            let s = st.sender.as_ref().unwrap();
            assert_eq!(s.stream, "AudioBridge");
            assert_eq!(s.name.as_deref(), Some("steamdeck"));
            assert_eq!(st.hint_buffer_ms, Some(20.0));
        }
        assert_eq!(lock(&playout).target_ms(), 20.0);
        assert!(lock(&playout).buffered_ms() > 70.0);

        std::thread::sleep(SENDER_TIMEOUT + Duration::from_millis(400));
        assert!(!lock(&shared.status).playing());
        assert!(lock(&shared.status).last_sender.is_some());
        listener.stop();
        assert!(lock(&shared.status).listening_on.is_none());
    }

    #[test]
    fn rejects_a_buffer_hint_that_is_not_a_number() {
        let playout = Arc::new(Mutex::new(Playout::new(40.0, true)));
        let shared = Arc::new(Shared::new(playout.clone(), Delay::Auto, true));
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let listener = start(shared.clone(), "127.0.0.1".into(), port);
        std::thread::sleep(Duration::from_millis(150));
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .set_read_timeout(Some(Duration::from_millis(400)))
            .unwrap();
        let target = format!("127.0.0.1:{port}");
        for (frame, hint) in ["nan", "inf", "-5", "100000"].iter().enumerate() {
            let comment = format!("audiobridge=1;buffer={hint}");
            sender
                .send_to(&ping(frame as u32, &comment), &target)
                .unwrap();
            let mut buf = [0u8; 1024];
            let _ = sender.recv_from(&mut buf);
        }
        assert_eq!(lock(&shared.status).hint_buffer_ms, None);
        assert_eq!(lock(&playout).target_ms(), 40.0);
        listener.stop();
    }

    #[test]
    fn discovery_can_be_switched_off() {
        let playout = Arc::new(Mutex::new(Playout::new(40.0, true)));
        let shared = Arc::new(Shared::new(playout, Delay::Auto, false));
        let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let listener = start(shared, "127.0.0.1".into(), port);
        std::thread::sleep(Duration::from_millis(150));
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender
            .set_read_timeout(Some(Duration::from_millis(400)))
            .unwrap();
        sender
            .send_to(&ping(1, ""), format!("127.0.0.1:{port}"))
            .unwrap();
        let mut buf = [0u8; 1024];
        assert!(sender.recv_from(&mut buf).is_err());
        listener.stop();
    }

    #[test]
    fn port_in_use_is_reported() {
        let blocker = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = blocker.local_addr().unwrap().port();
        let shared = Arc::new(Shared::new(
            Arc::new(Mutex::new(Playout::new(40.0, true))),
            Delay::Auto,
            true,
        ));
        let listener = start(shared.clone(), "127.0.0.1".into(), port);
        std::thread::sleep(Duration::from_millis(200));
        assert!(lock(&shared.status)
            .bind_error
            .as_deref()
            .unwrap_or("")
            .contains("already used"));
        listener.stop();
    }
}
