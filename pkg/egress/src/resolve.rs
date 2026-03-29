//! DNS resolution for egress allow rules.
//!
//! Resolves domain names from AGENT.toml network.allow lists to IP addresses
//! that can be used in nftables rules.

use std::collections::BTreeSet;
use std::net::IpAddr;

/// Resolve a list of domain names to IP addresses.
///
/// Uses the system resolver. Domains that fail to resolve are logged and skipped.
/// Returns a set of unique IPs across all domains.
pub fn resolve_domains(domains: &[String]) -> BTreeSet<IpAddr> {
    let mut ips = BTreeSet::new();

    for domain in domains {
        // If it's already an IP address, use it directly
        if let Ok(ip) = domain.parse::<IpAddr>() {
            ips.insert(ip);
            continue;
        }

        // Resolve via system DNS (uses /etc/resolv.conf)
        match std::net::ToSocketAddrs::to_socket_addrs(&(domain.as_str(), 0)) {
            Ok(addrs) => {
                let mut count = 0;
                for addr in addrs {
                    ips.insert(addr.ip());
                    count += 1;
                }
                if count == 0 {
                    eprintln!("egress: DNS resolved 0 addresses for '{domain}'");
                }
            }
            Err(e) => {
                eprintln!("egress: DNS resolution failed for '{domain}': {e}");
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
