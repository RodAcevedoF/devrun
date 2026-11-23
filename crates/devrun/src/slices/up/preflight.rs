use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

use anyhow::bail;
use devrun_supervisor::LaunchSpec;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
const LOOPBACKS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

pub fn ensure_ports_free(specs: &[LaunchSpec]) -> anyhow::Result<()> {
    for spec in specs {
        for &port in &spec.ports {
            if accepts(port) {
                bail!("port {port} is already in use (needed by {})", spec.id);
            }
        }
    }
    Ok(())
}

fn accepts(port: u16) -> bool {
    LOOPBACKS.into_iter().any(|address| {
        TcpStream::connect_timeout(&SocketAddr::new(address, port), CONNECT_TIMEOUT).is_ok()
    })
}
