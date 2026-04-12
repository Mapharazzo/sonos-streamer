use if_addrs::IfAddr;

use local_ip_address::local_ip;
use std::net::IpAddr;

use std::net::UdpSocket;

/// `get_local_address` - get the local ip address, return an `Option<String>`. when it fails, return `None`.

pub fn get_local_addr() -> Result<IpAddr, local_ip_address::Error> {
    local_ip()
}

pub fn get_interfaces() -> Vec<String> {
    let mut interfaces: Vec<String> = Vec::new();
    let ifaces = if_addrs::get_if_addrs().expect("could not get interfaces");
    ifaces
        .iter()
        .filter(|iface| matches!(iface.addr, IfAddr::V4(..)))
        .for_each(|iface| interfaces.push(iface.addr.ip().to_string()));
    interfaces
}
