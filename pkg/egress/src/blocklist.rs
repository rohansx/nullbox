//! Blocklist definitions for the Egress network controller.
//!
//! Hardcoded for v0.1. Future versions will support dynamic updates via OTA.

/// Cloud metadata endpoint CIDRs. These are used by cloud providers
/// (AWS, GCP, Azure, DigitalOcean) to expose instance metadata.
/// An agent reaching these can steal IAM credentials.
pub const CLOUD_METADATA_CIDRS: &[&str] = &[
    "169.254.169.254/32", // AWS, GCP, Azure, DO instance metadata
    "100.100.100.200/32", // Alibaba Cloud metadata
    "fd00:ec2::254/128",  // AWS IMDSv2 IPv6
];

/// RFC-1918 private address ranges. Agents should not be able to
/// reach internal network resources.
pub const RFC1918_CIDRS: &[&str] = &[
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
];

/// Link-local addresses (beyond cloud metadata).
pub const LINK_LOCAL_CIDRS: &[&str] = &[
    "169.254.0.0/16",
    "fe80::/10",
];

/// Check if an IP address falls within any blocked range.
/// For v0.1, this is a simple string-based check. Future versions
/// will use proper CIDR matching via the ipnet crate.
pub fn is_blocked(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 10.0.0.0/8
            if octets[0] == 10 {
                return true;
            }
            // 172.16.0.0/12
            if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                return true;
            }
            // 192.168.0.0/16
            if octets[0] == 192 && octets[1] == 168 {
                return true;
            }
            // 169.254.0.0/16 (link-local, includes cloud metadata)
            if octets[0] == 169 && octets[1] == 254 {
                return true;
            }
            // 100.100.100.200 (Alibaba)
            if octets == [100, 100, 100, 200] {
                return true;
            }
            false
        }
        IpAddr::V6(_v6) => {
            // For v0.1, block all IPv6 from agents. Simplifies the firewall.
            // Future: proper IPv6 policy.
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    #[test]
    fn blocks_cloud_metadata() {
        let ip: IpAddr = "169.254.169.254".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn blocks_rfc1918_10() {
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn blocks_rfc1918_172() {
        let ip: IpAddr = "172.16.0.1".parse().unwrap();
        assert!(is_blocked(&ip));
        let ip: IpAddr = "172.31.255.255".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn blocks_rfc1918_192() {
        let ip: IpAddr = "192.168.1.1".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn allows_public_ip() {
        let ip: IpAddr = "104.18.0.1".parse().unwrap();
        assert!(!is_blocked(&ip));
    }

    #[test]
    fn allows_cloudflare_dns() {
        let ip: IpAddr = "1.1.1.1".parse().unwrap();
        assert!(!is_blocked(&ip));
    }

    #[test]
    fn blocks_alibaba_metadata() {
        let ip: IpAddr = "100.100.100.200".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn blocks_ipv6() {
        let ip: IpAddr = "2001:db8::1".parse().unwrap();
        assert!(is_blocked(&ip));
    }

    #[test]
    fn does_not_block_172_outside_range() {
        let ip: IpAddr = "172.32.0.1".parse().unwrap();
        assert!(!is_blocked(&ip));
    }
}
