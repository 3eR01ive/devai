//! The machine's local addresses and the network-change watchdog
//! (REQ-LAN-03, REQ-LAN-08).
//!
//! Only addresses of THIS machine in the home segment are candidates; the
//! listener never binds `0.0.0.0` and the access never reaches the outside
//! world through a tunnel or a proxy.

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use crate::lan::state::LanAccess;

/// How often the watchdog re-reads the interface list (REQ-LAN-08). The
/// acceptance wording allows «не позже минуты» for a vanished phone; the
/// network itself is checked far quicker because an honest «сеть изменилась»
/// beats a silently dead server.
pub const NETWORK_POLL: Duration = Duration::from_secs(3);

/// Every IPv4 address of this machine that a phone in the home segment could
/// reach. Loopback and link-local are excluded: they are not «домашняя сеть».
#[cfg(unix)]
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() {
                let sa = &*ifa.ifa_addr;
                if sa.sa_family as i32 == libc::AF_INET {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                    let up = ifa.ifa_flags & libc::IFF_UP as u32 != 0;
                    let loopback = ifa.ifa_flags & libc::IFF_LOOPBACK as u32 != 0;
                    if up && !loopback && !ip.is_link_local() {
                        out.push(ip);
                    }
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

/// Outside unix there is no interface enumeration without extra crates; the
/// access honestly refuses to enable (REQ-LAN-03 AC4) instead of guessing an
/// address. The operating platform of this build is a unix desktop.
#[cfg(not(unix))]
pub fn local_ipv4s() -> Vec<Ipv4Addr> {
    Vec::new()
}

/// The address the access binds to: the home-segment address of the machine.
/// Private ranges first — a phone on the same Wi-Fi reaches them; the rest is
/// an honest attempt on unusual home networks. `None` = refuse to enable
/// (REQ-LAN-03 AC4), never a made-up address.
pub fn pick_lan_address(ips: &[Ipv4Addr]) -> Option<Ipv4Addr> {
    // Home routers overwhelmingly hand out 192.168/16; 10/8 is the next
    // common home/VPN plan. 172.16/12 is where container bridges (docker0 =
    // 172.17.0.1) live, so it goes AFTER the home-looking plans — the phone
    // on the Wi-Fi cannot reach a bridge address.
    let rank = |ip: &Ipv4Addr| -> u8 {
        if ip.is_loopback() || ip.is_link_local() {
            return u8::MAX;
        }
        let o = ip.octets();
        match o {
            [192, 168, _, _] => 0,
            [10, _, _, _] => 1,
            [172, 16..=31, _, _] => 3,
            _ => 2,
        }
    };
    ips.iter()
        .filter(|ip| rank(ip) != u8::MAX)
        .min_by_key(|ip| rank(ip))
        .copied()
}

/// Watch the bound address: when it disappears the access is switched off
/// HONESTLY with its reason and never rebound by itself (REQ-LAN-08).
pub fn spawn_watchdog(lan: Arc<LanAccess>, bound_ip: Ipv4Addr) {
    let generation = lan.generation();
    std::thread::Builder::new()
        .name("devai-lan-watch".into())
        .spawn(move || {
            loop {
                std::thread::sleep(NETWORK_POLL);
                if lan.generation() != generation {
                    return; // the access was disabled or restarted: this run is over
                }
                if lan.stop_if_network_changed(&local_ipv4s()) {
                    eprintln!("devai-ui: LAN-адрес {bound_ip} пропал — доступ выключен");
                    return;
                }
                if !lan.is_on() {
                    return;
                }
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lan_address_prefers_the_home_segment() {
        let ips: Vec<Ipv4Addr> = ["100.64.0.1", "192.168.1.20", "10.1.2.3"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            pick_lan_address(&ips),
            Some("192.168.1.20".parse().unwrap())
        );
        // A container bridge (docker0 = 172.17.0.1) is not a home address:
        // the Wi-Fi plan wins even when the bridge is enumerated first.
        let with_docker: Vec<Ipv4Addr> = ["172.17.0.1", "10.1.2.3", "127.0.0.1"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        assert_eq!(
            pick_lan_address(&with_docker),
            Some("10.1.2.3".parse().unwrap())
        );
        assert_eq!(pick_lan_address(&[]), None);
    }
}
