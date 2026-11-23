use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const LOOPBACKS: [IpAddr; 2] = [
    IpAddr::V4(Ipv4Addr::LOCALHOST),
    IpAddr::V6(Ipv6Addr::LOCALHOST),
];

pub(crate) struct HealthProbe(JoinHandle<()>);

impl HealthProbe {
    pub fn spawn(
        ports: Vec<u16>,
        interval: Duration,
        on_ready: impl FnOnce() + Send + 'static,
    ) -> Self {
        Self(tokio::spawn(async move {
            while !all_accept(&ports).await {
                sleep(interval).await;
            }
            on_ready();
        }))
    }
}

impl Drop for HealthProbe {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn all_accept(ports: &[u16]) -> bool {
    for &port in ports {
        if !accepts(port).await {
            return false;
        }
    }
    true
}

async fn accepts(port: u16) -> bool {
    for address in LOOPBACKS {
        if let Ok(Ok(_)) = timeout(CONNECT_TIMEOUT, TcpStream::connect((address, port))).await {
            return true;
        }
    }
    false
}
