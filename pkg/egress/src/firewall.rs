//! nftables rule generation for the Egress network controller.
//!
//! Generates an atomic nftables ruleset that enforces default-deny
//! with per-agent allow rules.

use crate::blocklist;
use std::collections::BTreeSet;
use std::net::IpAddr;

/// An nftables ruleset represented as a string ready for `nft -f`.
#[derive(Debug, Clone)]
pub struct Ruleset {
    pub content: String,
}

/// Per-agent network allow entry.
#[derive(Debug, Clone)]
pub struct AgentAllowRule {
    pub agent_name: String,
    pub tap_device: String,
    pub allowed_ips: BTreeSet<IpAddr>,
}

/// Generate the complete nftables ruleset.
///
/// Structure:
/// 1. Flush existing rules
/// 2. Create nullbox table
/// 3. Base chains: input (drop), forward (drop), output (accept for host)
/// 4. Block cloud metadata + RFC-1918 from agent interfaces
/// 5. Allow established/related
/// 6. Per-agent allow rules
pub fn generate_ruleset(agent_rules: &[AgentAllowRule]) -> Ruleset {
    let mut rules = String::new();

    // Flush and recreate
    rules.push_str("flush ruleset\n\n");
    rules.push_str("table inet nullbox {\n");

    // Forward chain — default deny for agent traffic
    rules.push_str("  chain forward {\n");
    rules.push_str(
        "    type filter hook forward priority 0; policy drop;\n",
    );
    rules.push_str("\n");

    // Allow established/related connections
    rules.push_str(
        "    ct state established,related accept\n",
    );
    rules.push_str("\n");

    // Block cloud metadata from all agent interfaces
    for cidr in blocklist::CLOUD_METADATA_CIDRS {
        rules.push_str(&format!(
            "    ip daddr {cidr} drop comment \"block cloud metadata\"\n"
        ));
    }
    rules.push_str("\n");

    // Block RFC-1918 from agent interfaces (not from host)
    for cidr in blocklist::RFC1918_CIDRS {
        for agent in agent_rules {
            rules.push_str(&format!(
                "    iifname \"{}\" ip daddr {} drop comment \"block RFC-1918 from {}\"\n",
                agent.tap_device, cidr, agent.agent_name
            ));
        }
    }
    rules.push_str("\n");

    // Per-agent allow rules
    for agent in agent_rules {
        rules.push_str(&format!(
            "    # Agent: {}\n",
            agent.agent_name
        ));
        for ip in &agent.allowed_ips {
            rules.push_str(&format!(
                "    iifname \"{}\" ip daddr {} accept\n",
                agent.tap_device, ip
            ));
        }
        rules.push_str("\n");
    }

    rules.push_str("  }\n"); // end forward chain

    // Output chain — allow host traffic (nulld, egress itself)
    rules.push_str("\n  chain output {\n");
    rules.push_str(
        "    type filter hook output priority 0; policy accept;\n",
    );
    rules.push_str("  }\n");

    // Input chain — drop unsolicited inbound
    rules.push_str("\n  chain input {\n");
    rules.push_str(
        "    type filter hook input priority 0; policy drop;\n",
    );
    rules.push_str("    iif lo accept\n");
    rules.push_str(
        "    ct state established,related accept\n",
    );
    rules.push_str("  }\n");

    rules.push_str("}\n"); // end table

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
            allowed_ips: BTreeSet::from([
                "104.18.0.1".parse().unwrap(),
            ]),
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
        // Host output chain should accept by default
        assert!(ruleset.content.contains("chain output"));
        assert!(ruleset.content.contains("policy accept"));
    }
}
