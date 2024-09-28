use std::env;
use std::net::Ipv4Addr;
use ipnetwork::Ipv4Network;
use tokio::process::Command;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;
use std::sync::Arc;
use serde::{Serialize, Deserialize};
use std::process::Stdio;

#[derive(Serialize, Deserialize)]
struct PingResult {
    ip: Ipv4Addr,
    reachable: bool,
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() != 4 {
        eprintln!("Usage: {} <network_address> <subnet_mask> <range>", args[0]);
        eprintln!("Example: {} 192.168.1.0 /24 1-254", args[0]);
        return;
    }

    let network_address = &args[1];
    let subnet_mask = &args[2];
    let range = &args[3];

    // Parse network address
    match network_address.parse::<Ipv4Addr>() {
        Ok(_) => {},
        Err(_) => {
            eprintln!("Invalid network address");
            return;
        }
    };

    // Parse subnet mask
    let network: Ipv4Network = if subnet_mask.starts_with('/') {
        // CIDR notation
        let cidr_notation = format!("{}/{}", network_address, &subnet_mask[1..]);
        match cidr_notation.parse() {
            Ok(net) => net,
            Err(_) => {
                eprintln!("Invalid CIDR notation");
                return;
            }
        }
    } else {
        // Subnet mask in dot-decimal notation
        let subnet_mask_ip: Ipv4Addr = match subnet_mask.parse() {
            Ok(ip) => ip,
            Err(_) => {
                eprintln!("Invalid subnet mask");
                return;
            }
        };

        // Compute CIDR prefix length from subnet mask
        let prefix_len = subnet_mask_ip.octets().iter().fold(0, |acc, &b| acc + b.count_ones());
        let cidr_notation = format!("{}/{}", network_address, prefix_len);
        match cidr_notation.parse() {
            Ok(net) => net,
            Err(_) => {
                eprintln!("Invalid subnet mask");
                return;
            }
        }
    };

    // Parse range
    let range_parts: Vec<&str> = range.split('-').collect();
    if range_parts.len() != 2 {
        eprintln!("Invalid range. Example of valid range: 1-254");
        return;
    }

    let start: u8 = match range_parts[0].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Invalid start of range");
            return;
        }
    };

    let end: u8 = match range_parts[1].parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("Invalid end of range");
            return;
        }
    };

    if start > end {
        eprintln!("Start of range must be less than or equal to end of range");
        return;
    }

    // Generate IP addresses in the range within the subnet
    let mut ips_to_ping = Vec::new();

    for ip in network.iter() {
        let octets = ip.octets();
        let last_octet = octets[3];
        if last_octet >= start && last_octet <= end {
            ips_to_ping.push(ip);
        }
    }

    // Set a concurrency limit
    let max_concurrent_pings = 100; // Adjust this number as needed
    let semaphore = Arc::new(Semaphore::new(max_concurrent_pings));

    // Ping the IP addresses concurrently with a limit
    let mut futures = FuturesUnordered::new();

    // Create a channel to collect results
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    for ip in ips_to_ping {
        let sem_clone = semaphore.clone();
        let tx_clone = tx.clone();
        futures.push(tokio::spawn(async move {
            // Acquire a permit before starting the ping
            let _permit = sem_clone.acquire().await;
            match ping(ip).await {
                Ok((ip, reachable)) => {
                    tx_clone.send(PingResult { ip, reachable }).unwrap();
                }
                Err(e) => {
                    eprintln!("Error pinging {}: {}", ip, e);
                }
            }
            // The permit is automatically released when `_permit` goes out of scope
        }));
    }

    // Drop the original sender so the receiver will know when all messages have been sent
    drop(tx);

    // Collect results
    let mut results = Vec::new();
    while let Some(ping_result) = rx.recv().await {
        results.push(ping_result);
    }

    // Wait for all tasks to complete
    while let Some(_) = futures.next().await {}

    // Sort the results by IP address
    results.sort_by_key(|r| r.ip);

    // Output the results in JSON format
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}

async fn ping(ip: Ipv4Addr) -> Result<(Ipv4Addr, bool), std::io::Error> {
    // Adjust the ping command based on the operating system
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("ping");
        c.args(&["-n", "1", "-w", "1000", &ip.to_string()]);
        c
    } else {
        let mut c = Command::new("ping");

        // Add the '-q' option for quiet output on Unix-like systems
        c.args(&["-c", "1", "-W", "1", "-q", &ip.to_string()]);
        c
    };

    // Suppress the standard output and standard error
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Run the command and capture the exit status
    let status = cmd.status().await?;

    Ok((ip, status.success()))
}
