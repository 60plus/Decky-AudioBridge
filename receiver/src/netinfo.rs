//! Which addresses this PC has, and which one is the main one.

use std::net::{IpAddr, Ipv4Addr, UdpSocket};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub ip: Ipv4Addr,
}

pub fn adapters() -> Vec<Adapter> {
    let mut list: Vec<Adapter> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(Adapter {
                name: iface.name,
                ip,
            }),
            _ => None,
        })
        .collect();
    list.sort_by_key(|a| (!a.ip.is_private(), a.ip));
    list.dedup_by_key(|a| a.ip);
    list
}

/// The address Windows would use to reach the rest of the network. No packet
/// is sent: connecting a UDP socket only asks the routing table.
pub fn main_ip() -> Option<Ipv4Addr> {
    for probe in ["192.0.2.1:9", "8.8.8.8:53"] {
        if let Ok(sock) = UdpSocket::bind("0.0.0.0:0") {
            if sock.connect(probe).is_ok() {
                if let Ok(addr) = sock.local_addr() {
                    if let IpAddr::V4(ip) = addr.ip() {
                        if !ip.is_unspecified() && !ip.is_loopback() {
                            return Some(ip);
                        }
                    }
                }
            }
        }
    }
    adapters()
        .into_iter()
        .map(|a| a.ip)
        .find(|ip| ip.is_private())
}

/// Senders we accept by default: anything on a private or local network.
pub fn is_local_sender(ip: Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xC0) == 64)
}

pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "Receiver".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_senders() {
        assert!(is_local_sender(Ipv4Addr::new(192, 168, 0, 116)));
        assert!(is_local_sender(Ipv4Addr::new(10, 1, 2, 3)));
        assert!(is_local_sender(Ipv4Addr::new(100, 64, 0, 1)));
        assert!(!is_local_sender(Ipv4Addr::new(8, 8, 8, 8)));
        assert!(!is_local_sender(Ipv4Addr::new(100, 128, 0, 1)));
    }
}
