// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! The network a phone reaches this computer on (isled's share/net.rs, from Windows' adapters): the IPv4 address
//! of the adapter carrying the default route, a private one first, Wi-Fi first; and how far it is trusted.

use std::net::{IpAddr, Ipv4Addr};

use serde::Serialize;

/// One IPv4 network the computer is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lan {
    pub ip: Ipv4Addr,
    pub prefix: u8,
    pub interface: String,
}

impl Lan {
    /// Whether `peer` is on this network (a phone on the same Wi-Fi), or is this computer.
    pub fn contains(&self, peer: IpAddr) -> bool {
        let IpAddr::V4(peer) = peer.to_canonical() else {
            return false;
        };
        let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
        u32::from(peer) & mask == u32::from(self.ip) & mask
    }
}

/// How far the network looks to be trusted, as the island says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum Trust {
    Home,
    Open,
    Public,
    Unknown,
}

/// An address only a local network uses (RFC 1918, shared or link-local).
pub fn is_private(ip: Ipv4Addr) -> bool {
    let shared = ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]);
    ip.is_private() || ip.is_link_local() || shared
}

/// Adapters of VMs, containers and VPNs, by their names: a phone is never on these.
const VIRTUAL: &[&str] = &[
    "virtual", "vmware", "virtualbox", "hyper-v", "vethernet", "wsl", "docker", "tailscale", "zerotier", "wireguard",
    "openvpn", "tap-", "tun", "vpn", "loopback", "bluetooth",
];

/// An adapter's address, and whether it is Wi-Fi and carries the default route.
#[derive(Debug, Clone)]
pub struct Adapter {
    pub lan: Lan,
    pub wifi: bool,
    pub default: bool,
    pub name: String,
}

/// The address a phone on the same network reaches this computer at. None without a network.
pub fn lan() -> Option<Lan> {
    pick(imp::adapters())
}

fn pick(adapters: Vec<Adapter>) -> Option<Lan> {
    let mut usable: Vec<Adapter> = adapters
        .into_iter()
        .filter(|adapter| {
            let name = adapter.name.to_lowercase();
            !adapter.lan.ip.is_loopback() && !VIRTUAL.iter().any(|word| name.contains(word))
        })
        .collect();
    // The default route's first, then a private address, then Wi-Fi.
    usable.sort_by_key(|adapter| (!adapter.default, !is_private(adapter.lan.ip), !adapter.wifi));
    usable.into_iter().next().map(|adapter| adapter.lan)
}

/// This computer's own IPv4 addresses: a connection from one of them comes from a program here, not a phone.
pub fn own_addresses() -> Vec<IpAddr> {
    imp::adapters().into_iter().map(|adapter| IpAddr::V4(adapter.lan.ip)).collect()
}

/// A public address is reachable from anywhere; a Wi-Fi without a password lets others nearby listen in.
pub fn trust(ip: Ipv4Addr) -> Trust {
    if !is_private(ip) {
        return Trust::Public;
    }
    match imp::wifi_secured() {
        Some(false) => Trust::Open,
        _ => Trust::Home,
    }
}

#[cfg(windows)]
mod imp {
    use std::net::Ipv4Addr;

    use windows::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_SUCCESS, HANDLE};
    use windows::Win32::NetworkManagement::IpHelper::{
        GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses,
        GetBestInterface, IP_ADAPTER_ADDRESSES_LH,
    };
    use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows::Win32::NetworkManagement::WiFi::{
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
        WlanOpenHandle, WlanQueryInterface, wlan_intf_opcode_current_connection,
        wlan_interface_state_connected,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN};

    use super::{Adapter, Lan};

    const IF_TYPE_IEEE80211: u32 = 71;

    pub fn adapters() -> Vec<Adapter> {
        let mut found = Vec::new();
        // SAFETY: the adapters' list read into a buffer of the size the call asks for, walked while it lives.
        unsafe {
            // The interface the default route leaves by (to any address on the internet).
            let mut best = 0u32;
            let _ = GetBestInterface(u32::from_ne_bytes([8, 8, 8, 8]), &mut best);
            let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
            let mut size = 16 * 1024u32;
            let mut buffer: Vec<u64> = Vec::new();
            for _ in 0..3 {
                buffer = vec![0u64; size as usize / 8 + 1];
                let got = GetAdaptersAddresses(
                    u32::from(AF_INET.0),
                    flags,
                    None,
                    Some(buffer.as_mut_ptr().cast()),
                    &mut size,
                );
                if got == ERROR_SUCCESS.0 {
                    break;
                }
                if got != ERROR_BUFFER_OVERFLOW.0 {
                    return found;
                }
            }
            let mut adapter = buffer.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
            while !adapter.is_null() {
                let a = &*adapter;
                adapter = a.Next;
                if a.OperStatus != IfOperStatusUp {
                    continue;
                }
                let name = format!(
                    "{} {}",
                    a.FriendlyName.to_string().unwrap_or_default(),
                    a.Description.to_string().unwrap_or_default()
                );
                let mut unicast = a.FirstUnicastAddress;
                while !unicast.is_null() {
                    let u = &*unicast;
                    unicast = u.Next;
                    let socket = u.Address.lpSockaddr;
                    if socket.is_null() || (*socket).sa_family != AF_INET {
                        continue;
                    }
                    let address = &*(socket as *const SOCKADDR_IN);
                    let ip = Ipv4Addr::from(u32::from_be(address.sin_addr.S_un.S_addr));
                    found.push(Adapter {
                        lan: Lan { ip, prefix: u.OnLinkPrefixLength, interface: name.clone() },
                        wifi: a.IfType == IF_TYPE_IEEE80211,
                        default: a.Anonymous1.Anonymous.IfIndex == best,
                        name: name.clone(),
                    });
                }
            }
        }
        found
    }

    /// Whether the Wi-Fi connected now has a password; None on a cable, or if Windows could not say.
    pub fn wifi_secured() -> Option<bool> {
        // SAFETY: a WLAN client handle, its interfaces and their current connections, each freed once.
        unsafe {
            let mut version = 0u32;
            let mut client = HANDLE::default();
            if WlanOpenHandle(2, None, &mut version, &mut client) != ERROR_SUCCESS.0 {
                return None;
            }
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            let mut secured = None;
            if WlanEnumInterfaces(client, None, &mut list) == ERROR_SUCCESS.0 && !list.is_null() {
                let count = (*list).dwNumberOfItems as usize;
                let infos = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), count);
                for info in infos.iter().filter(|info| info.isState == wlan_interface_state_connected) {
                    let mut size = 0u32;
                    let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
                    if WlanQueryInterface(
                        client,
                        &info.InterfaceGuid,
                        wlan_intf_opcode_current_connection,
                        None,
                        &mut size,
                        &mut data,
                        None,
                    ) == ERROR_SUCCESS.0
                        && !data.is_null()
                    {
                        let attributes = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                        secured = Some(attributes.wlanSecurityAttributes.bSecurityEnabled.as_bool());
                        WlanFreeMemory(data);
                    }
                }
                WlanFreeMemory(list.cast());
            }
            WlanCloseHandle(client, None);
            secured
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn adapters() -> Vec<super::Adapter> {
        Vec::new()
    }
    pub fn wifi_secured() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(ip: &str, name: &str, wifi: bool, default: bool) -> Adapter {
        Adapter { lan: Lan { ip: ip.parse().unwrap(), prefix: 24, interface: name.into() }, wifi, default, name: name.into() }
    }

    #[test]
    fn the_default_routes_adapter_wins_and_virtual_ones_never() {
        let picked = pick(vec![
            adapter("172.20.0.1", "vEthernet (WSL)", false, false),
            adapter("10.8.0.2", "OpenVPN TAP-Windows6", false, false),
            adapter("192.168.1.20", "Wi-Fi Intel Wireless", true, true),
            adapter("192.168.56.1", "Ethernet 2 VirtualBox Host-Only", false, false),
        ]);
        assert_eq!(picked.unwrap().ip, "192.168.1.20".parse::<Ipv4Addr>().unwrap());
        assert!(pick(vec![adapter("172.20.0.1", "vEthernet (WSL)", false, true)]).is_none());
    }

    #[test]
    fn a_phone_on_the_same_wifi_is_on_the_network() {
        let lan = Lan { ip: "192.168.1.20".parse().unwrap(), prefix: 24, interface: "Wi-Fi".into() };
        assert!(lan.contains("192.168.1.77".parse().unwrap()));
        assert!(!lan.contains("192.168.2.77".parse().unwrap()));
    }
}
