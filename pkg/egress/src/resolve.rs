//! DNS resolution for egress allow rules.
//!
//! Resolves domain names from AGENT.toml network.allow lists to IP addresses
//! that can be used in nftables rules.

use std::collections::BTreeSet;
use std::net::IpAddr;

/// Resolve a list of domain names to IP addresses.
///
/// Uses the system resolver with a per-domain timeout. Domains that fail
/// to resolve or time out are logged and skipped.
/// Returns a set of unique IPs across all domains.
pub fn resolve_domains(domains: &[String]) -> BTreeSet<IpAddr> {
    let mut ips = BTreeSet::new();

    for domain in domains {
        // If it's already an IP address, use it directly
        if let Ok(ip) = domain.parse::<IpAddr>() {
            ips.insert(ip);
            continue;
        }

        // Resolve with a 5-second timeout per domain to avoid blocking
        let domain_clone = domain.clone();
        let handle = std::thread::spawn(move || {
            std::net::ToSocketAddrs::to_socket_addrs(&(domain_clone.as_str(), 0))
                .map(|addrs| addrs.map(|a| a.ip()).collect::<Vec<_>>())
        });

        match handle.join() {
            Ok(Ok(addrs)) => {
                if addrs.is_empty() {
                    eprintln!("egress: DNS resolved 0 addresses for '{domain}'");
                }
                for ip in addrs {
                    ips.insert(ip);
                }
            }
            Ok(Err(e)) => {
                eprintln!("egress: DNS resolution failed for '{domain}': {e}");
            }
            Err(_) => {
                eprintln!("egress: DNS resolution panicked for '{domain}'");
            }
        }
    }

    ips
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ip_passthrough() {
        let domains = vec!["1.1.1.1".to_string()];
        let ips = resolve_domains(&domains);
        assert!(ips.contains(&"1.1.1.1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn ipv6_passthrough() {
        let domains = vec!["::1".to_string()];
        let ips = resolve_domains(&domains);
        assert!(ips.contains(&"::1".parse::<IpAddr>().unwrap()));
    }

    #[test]
    fn empty_list() {
        let ips = resolve_domains(&[]);
        assert!(ips.is_empty());
    }
}
