//! nftables rule generation for the Egress network controller.
//!
//! Generates an atomic nftables ruleset that enforces default-deny
//! with per-agent allow rules.
//!
//! With TSI networking, agent traffic exits through the host's output chain
//! (not forwarded through a tap device). We use the output chain with
//! IP allowlists to control what agents can reach.

use crate::blocklist;
use std::collections::BTreeSet;
use std::net::IpAddr;

/// An nftables ruleset represented as a string ready for `nft -f`.
#[derive(Debug, Clone)]
pub struct Ruleset {
    pub content: String,
}

/// Per-agent network allow entry (tap-based, legacy).
#[derive(Debug, Clone)]
pub struct AgentAllowRule {
    pub agent_name: String,
    pub tap_device: String,
    pub allowed_ips: BTreeSet<IpAddr>,
}

/// Per-agent allow entry for TSI mode.
#[derive(Debug, Clone)]
pub struct TsiAgentRule {
    pub agent_name: String,
    pub allowed_ips: BTreeSet<IpAddr>,
}

/// Generate the nftables ruleset for TSI-based networking.
///
/// Structure:
/// 1. Flush existing rules
/// 2. Create nullbox table
/// 3. Output chain: default deny for external, allow loopback + established
/// 4. Block cloud metadata + RFC-1918
/// 5. Allow resolved agent IPs
/// 6. Input chain: drop unsolicited inbound, allow established
pub fn generate_tsi_ruleset(agent_rules: &[TsiAgentRule]) -> Ruleset {
    let mut rules = String::new();

    rules.push_str("flush ruleset\n\n");
    rules.push_str("table inet nullbox {\n");

    // Output chain — controls all outbound traffic including TSI-proxied agent traffic
    rules.push_str("  chain output {\n");
    rules.push_str("    type filter hook output priority 0; policy drop;\n");
    rules.push_str("\n");

    // Always allow loopback (local services: ctxgraph, cage, egress sockets)
    rules.push_str("    oif lo accept\n");
    rules.push_str("\n");

    // Allow established/related (return traffic for allowed connections)
    rules.push_str("    ct state established,related accept\n");
    rules.push_str("\n");

    // Allow DNS (needed for resolution — TCP and UDP port 53)
    rules.push_str("    udp dport 53 accept comment \"allow DNS\"\n");
    rules.push_str("    tcp dport 53 accept comment \"allow DNS\"\n");
    rules.push_str("\n");

    // Block cloud metadata endpoints
    for cidr in blocklist::CLOUD_METADATA_CIDRS {
        rules.push_str(&format!(
            "    ip daddr {cidr} drop comment \"block cloud metadata\"\n"
        ));
    }
    rules.push_str("\n");

    // Block RFC-1918 (except loopback, already allowed above)
    for cidr in blocklist::RFC1918_CIDRS {
        rules.push_str(&format!(
            "    ip daddr {cidr} drop comment \"block private ranges\"\n"
        ));
    }
    rules.push_str("\n");

    // Per-agent allow rules — resolved IPs from AGENT.toml domains
    if agent_rules.is_empty() {
        rules.push_str("    # No agents registered — all external traffic blocked\n");
    } else {
        for agent in agent_rules {
            rules.push_str(&format!("    # Agent: {}\n", agent.agent_name));
            for ip in &agent.allowed_ips {
                rules.push_str(&format!(
                    "    ip daddr {} accept comment \"allow for {}\"\n",
                    ip, agent.agent_name
                ));
            }
            rules.push_str("\n");
        }
    }

    // Log + drop anything else
    rules.push_str("    log prefix \"nullbox-deny: \" drop\n");
    rules.push_str("  }\n"); // end output chain

    // Input chain — drop unsolicited inbound
    rules.push_str("\n  chain input {\n");
    rules.push_str("    type filter hook input priority 0; policy drop;\n");
    rules.push_str("    iif lo accept\n");
    rules.push_str("    ct state established,related accept\n");
    rules.push_str("  }\n");

    rules.push_str("}\n"); // end table

    Ruleset { content: rules }
}

/// Generate legacy forward-chain ruleset (for tap-device based networking).
pub fn generate_ruleset(agent_rules: &[AgentAllowRule]) -> Ruleset {
    let mut rules = String::new();

    rules.push_str("flush ruleset\n\n");
    rules.push_str("table inet nullbox {\n");

    rules.push_str("  chain forward {\n");
    rules.push_str("    type filter hook forward priority 0; policy drop;\n");
    rules.push_str("\n");
    rules.push_str("    ct state established,related accept\n");
    rules.push_str("\n");

    for cidr in blocklist::CLOUD_METADATA_CIDRS {
        rules.push_str(&format!(
            "    ip daddr {cidr} drop comment \"block cloud metadata\"\n"
        ));
    }
    rules.push_str("\n");

    for cidr in blocklist::RFC1918_CIDRS {
        for agent in agent_rules {
            rules.push_str(&format!(
                "    iifname \"{}\" ip daddr {} drop comment \"block RFC-1918 from {}\"\n",
                agent.tap_device, cidr, agent.agent_name
            ));
        }
    }
    rules.push_str("\n");

    for agent in agent_rules {
        rules.push_str(&format!("    # Agent: {}\n", agent.agent_name));
        for ip in &agent.allowed_ips {
            rules.push_str(&format!(
                "    iifname \"{}\" ip daddr {} accept\n",
                agent.tap_device, ip
            ));
        }
        rules.push_str("\n");
    }

    rules.push_str("  }\n");

    rules.push_str("\n  chain output {\n");
    rules.push_str("    type filter hook output priority 0; policy accept;\n");
    rules.push_str("  }\n");

    rules.push_str("\n  chain input {\n");
    rules.push_str("    type filter hook input priority 0; policy drop;\n");
    rules.push_str("    iif lo accept\n");
    rules.push_str("    ct state established,related accept\n");
    rules.push_str("  }\n");

    rules.push_str("}\n");

    Ruleset { content: rules }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_agent_rules_produces_default_deny() {
        let ruleset = generate_ruleset(&[]);
        assert!(ruleset.content.contains("policy drop"));
        assert!(ruleset.content.contains("flush ruleset"));
        assert!(ruleset.content.contains("169.254.169.254"));
    }

    #[test]
    fn agent_allow_rule_appears_in_output() {
        let rules = vec![AgentAllowRule {
            agent_name: "researcher".to_string(),
            tap_device: "tap-researcher".to_string(),
            allowed_ips: BTreeSet::from(["104.18.0.1".parse().unwrap()]),
        }];

        let ruleset = generate_ruleset(&rules);
        assert!(ruleset.content.contains("tap-researcher"));
        assert!(ruleset.content.contains("104.18.0.1"));
        assert!(ruleset.content.contains("# Agent: researcher"));
    }

    #[test]
    fn rfc1918_blocked_per_agent() {
        let rules = vec![AgentAllowRule {
            agent_name: "test".to_string(),
            tap_device: "tap-test".to_string(),
            allowed_ips: BTreeSet::new(),
        }];

        let ruleset = generate_ruleset(&rules);
        assert!(ruleset.content.contains("10.0.0.0/8"));
        assert!(ruleset.content.contains("172.16.0.0/12"));
        assert!(ruleset.content.contains("192.168.0.0/16"));
    }

    #[test]
    fn host_output_is_allowed() {
        let ruleset = generate_ruleset(&[]);
        assert!(ruleset.content.contains("chain output"));
        assert!(ruleset.content.contains("policy accept"));
    }

    // TSI mode tests

    #[test]
    fn tsi_empty_blocks_all_external() {
        let ruleset = generate_tsi_ruleset(&[]);
        assert!(ruleset.content.contains("policy drop"));
        assert!(ruleset.content.contains("oif lo accept"));
        assert!(ruleset.content.contains("No agents registered"));
    }

    #[test]
    fn tsi_allows_loopback_and_dns() {
        let ruleset = generate_tsi_ruleset(&[]);
        assert!(ruleset.content.contains("oif lo accept"));
        assert!(ruleset.content.contains("udp dport 53 accept"));
    }

    #[test]
    fn tsi_agent_ips_allowed() {
        let rules = vec![TsiAgentRule {
            agent_name: "researcher".to_string(),
            allowed_ips: BTreeSet::from([
                "104.18.0.1".parse().unwrap(),
                "104.18.0.2".parse().unwrap(),
            ]),
        }];

        let ruleset = generate_tsi_ruleset(&rules);
        assert!(ruleset.content.contains("104.18.0.1"));
        assert!(ruleset.content.contains("104.18.0.2"));
        assert!(ruleset.content.contains("# Agent: researcher"));
    }

    #[test]
    fn tsi_blocks_cloud_metadata() {
        let ruleset = generate_tsi_ruleset(&[]);
        assert!(ruleset.content.contains("169.254.169.254"));
        assert!(ruleset.content.contains("block cloud metadata"));
    }

    #[test]
    fn tsi_blocks_private_ranges() {
        let ruleset = generate_tsi_ruleset(&[]);
        assert!(ruleset.content.contains("10.0.0.0/8"));
        assert!(ruleset.content.contains("172.16.0.0/12"));
        assert!(ruleset.content.contains("192.168.0.0/16"));
    }

    #[test]
    fn tsi_logs_denied_traffic() {
        let ruleset = generate_tsi_ruleset(&[]);
        assert!(ruleset.content.contains("log prefix"));
        assert!(ruleset.content.contains("nullbox-deny"));
    }
}
