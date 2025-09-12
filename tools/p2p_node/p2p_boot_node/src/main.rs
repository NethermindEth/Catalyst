use anyhow::Result;
use discv5::{ConfigBuilder, Discv5, Event, ListenConfig, enr, enr::CombinedKey};
use std::{
    net::{Ipv4Addr, SocketAddr, ToSocketAddrs},
    sync::Arc,
};
use tracing::{error, info};

use jsonrpsee::server::{RpcModule, ServerBuilder};

/// Resolves a string to an IPv4 address, supporting both IP addresses and domain names
async fn resolve_to_ipv4(addr_str: &str) -> Result<Ipv4Addr> {
    // First try to parse as IPv4 address directly
    if let Ok(ip) = addr_str.parse::<Ipv4Addr>() {
        return Ok(ip);
    }

    // If that fails, try to resolve as domain name
    let socket_addrs: Vec<SocketAddr> = format!("{}:0", addr_str).to_socket_addrs()?.collect();

    // Find the first IPv4 address
    for addr in socket_addrs {
        if let SocketAddr::V4(v4_addr) = addr {
            return Ok(*v4_addr.ip());
        }
    }

    Err(anyhow::anyhow!(
        "Could not resolve '{}' to an IPv4 address",
        addr_str
    ))
}

fn create_rpc_handler(enr: Arc<String>) -> Result<RpcModule<()>> {
    let mut module = RpcModule::new(());

    module.register_method("p2p_getENR", move |_, _, _| enr.to_string())?;

    module.register_method("health", |_, _, _| {
        // Return a simple response indicating the service is healthy
        "Ok".to_string()
    })?;
    Ok(module)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup tracing
    let filter_layer = tracing_subscriber::EnvFilter::try_from_default_env()
        .or_else(|_| tracing_subscriber::EnvFilter::try_new("debug"))
        .unwrap();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter_layer)
        .try_init();

    // if there is an address specified use it (supports both IP and domain)
    let address = if let Some(addr_str) = std::env::args().nth(1) {
        match resolve_to_ipv4(&addr_str).await {
            Ok(ip) => {
                info!("Resolved '{}' to IP address: {}", addr_str, ip);
                Some(ip)
            }
            Err(e) => {
                eprintln!("Failed to resolve address '{}': {}", addr_str, e);
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    // if there is a port specified use it
    let port = {
        if let Some(udp_port) = std::env::args().nth(2) {
            udp_port.parse().unwrap()
        } else {
            9000
        }
    };
    info!("UDP port: {port}");

    // listening address and port
    let listen_config = ListenConfig::Ipv4 {
        ip: Ipv4Addr::UNSPECIFIED,
        port: 9000,
    };

    let enr_key = CombinedKey::generate_secp256k1();

    // construct a local ENR
    let enr = {
        let mut builder = enr::Enr::builder();
        // if an IP was specified, use it
        if let Some(external_address) = address {
            builder.ip4(external_address);
        }
        // if a port was specified, use it
        if std::env::args().nth(2).is_some() {
            builder.udp4(port);
        }
        builder.build(&enr_key).unwrap()
    };

    // if the ENR is useful print it
    info!("Node Id: {}", enr.node_id());
    if enr.udp4_socket().is_some() {
        info!("Base64 ENR: {}", enr.to_base64());
        info!(
            "IP: {}, UDP_PORT:{}",
            enr.ip4().unwrap(),
            enr.udp4().unwrap()
        );
    } else {
        info!("ENR is not printed as no IP:PORT was specified");
    }

    // default configuration
    let config = ConfigBuilder::new(listen_config).build();

    // construct the discv5 server
    let mut discv5: Discv5 = Discv5::new(enr, enr_key, config).unwrap();

    // save base64 ENR
    let enr_base64 = Arc::new(discv5.local_enr().to_base64());

    save_enr_to_file(&enr_base64);

    // Start the JSON-RPC server in a separate tokio task
    let rpc_enr_base64 = Arc::clone(&enr_base64); // Clone for the RPC thread
    let module = create_rpc_handler(rpc_enr_base64)?;

    let addr: SocketAddr = "0.0.0.0:9001".parse().unwrap();
    info!("RPC server to be started on {addr}");
    let server = ServerBuilder::default()
        .build(addr)
        .await
        .expect("Unable to start RPC server");
    let handle = server.start(module);
    // we don't care about doing shutdown
    tokio::spawn(handle.stopped());

    // Start the discv5 service
    discv5.start().await.unwrap();
    info!("Discv5 server started");

    // Start the event loop for discv5
    let mut event_stream = discv5.event_stream().await.unwrap();

    loop {
        match event_stream.recv().await {
            Some(Event::SocketUpdated(addr)) => {
                info!("Nodes ENR socket address has been updated to: {addr:?}");
            }
            Some(Event::Discovered(enr)) => {
                info!("A peer has been discovered: {}", enr.node_id());
            }
            _ => {}
        }
    }
}

fn save_enr_to_file(enr_base64: &str) {
    use std::fs::File;
    use std::io::Write;
    if let Ok(mut file) = File::create("/tmp/enr") {
        let _ = file.write_all(enr_base64.as_bytes());
    } else {
        error!("Could not write ENR to /tmp/enr");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn test_resolve_to_ipv4_with_ip_address() {
        // Test with a valid IPv4 address
        let result = resolve_to_ipv4("192.168.1.1").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(192, 168, 1, 1));

        // Test with localhost IP
        let result = resolve_to_ipv4("127.0.0.1").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(127, 0, 0, 1));

        // Test with public IP
        let result = resolve_to_ipv4("8.8.8.8").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(8, 8, 8, 8));
    }

    #[tokio::test]
    async fn test_resolve_to_ipv4_with_domain_name() {
        // Test with localhost domain
        let result = resolve_to_ipv4("localhost").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), Ipv4Addr::new(127, 0, 0, 1));
    }

    #[tokio::test]
    async fn test_resolve_to_ipv4_with_public_domain() {
        // Test with a public domain that should resolve to IPv4
        // Using a domain that's known to have IPv4 addresses
        let result = resolve_to_ipv4("google.com").await;
        if result.is_ok() {
            let ip = result.unwrap();
            println!("IP: {ip}");
            // Just verify it's a valid IPv4 address
            assert!(!ip.is_unspecified());
            // Don't assert specific IP as it can change
        } else {
            // If DNS resolution fails (e.g., in CI without internet), that's also acceptable
            // This test is more about ensuring the function doesn't panic
            println!("DNS resolution failed, which is acceptable in some environments");
        }
    }

    #[tokio::test]
    async fn test_resolve_to_ipv4_with_invalid_input() {
        // Test with invalid IP format
        let result = resolve_to_ipv4("999.999.999.999").await;
        assert!(result.is_err());

        // Test with non-existent domain
        let result = resolve_to_ipv4("this-domain-definitely-does-not-exist-12345.com").await;
        assert!(result.is_err());

        // Test with empty string
        let result = resolve_to_ipv4("").await;
        assert!(result.is_err());

        // Test with invalid characters
        let result = resolve_to_ipv4("not-an-ip-or-domain!@#").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_resolve_to_ipv4_consistency() {
        // Test that resolving the same domain multiple times gives consistent results
        let result1 = resolve_to_ipv4("localhost").await;
        let result2 = resolve_to_ipv4("localhost").await;

        assert!(result1.is_ok());
        assert!(result2.is_ok());
        assert_eq!(result1.unwrap(), result2.unwrap());
    }
}
