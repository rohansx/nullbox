//! Service definition and dependency resolution for nulld.

use std::collections::{HashMap, HashSet, VecDeque};

/// A service managed by nulld.
#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub name: String,
    pub binary: String,
    pub args: Vec<String>,
    pub depends_on: Vec<String>,
    pub restart: RestartPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    Always,
    OnFailure,
    Never,
}

/// Topological sort of services by dependency order.
/// Returns services in the order they should be started.
/// Returns an error if there is a dependency cycle.
pub fn resolve_start_order(services: &[ServiceDef]) -> Result<Vec<String>, DepError> {
    let names: HashSet<&str> = services.iter().map(|s| s.name.as_str()).collect();

    // Validate all dependencies exist
    for svc in services {
        for dep in &svc.depends_on {
            if !names.contains(dep.as_str()) {
                return Err(DepError::MissingDependency {
                    service: svc.name.clone(),
                    dependency: dep.clone(),
                });
            }
        }
    }

    // Kahn's algorithm for topological sort
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();

    for svc in services {
        in_degree.entry(svc.name.as_str()).or_insert(0);
        for dep in &svc.depends_on {
            *in_degree.entry(svc.name.as_str()).or_insert(0) += 1;
            dependents
                .entry(dep.as_str())
                .or_default()
                .push(svc.name.as_str());
        }
    }

    let mut queue: VecDeque<&str> = in_degree
        .iter()
        .filter(|(_, deg)| **deg == 0)
        .map(|(&name, _)| name)
        .collect();

    let mut order = Vec::new();

    while let Some(name) = queue.pop_front() {
        order.push(name.to_string());

        if let Some(deps) = dependents.get(name) {
            for &dep in deps {
                if let Some(deg) = in_degree.get_mut(dep) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep);
                    }
                }
            }
        }
    }

    if order.len() != services.len() {
        return Err(DepError::CycleDetected);
    }

    Ok(order)
}

/// Returns the reverse of start order (for shutdown).
pub fn resolve_stop_order(services: &[ServiceDef]) -> Result<Vec<String>, DepError> {
    let mut order = resolve_start_order(services)?;
    order.reverse();
    Ok(order)
}

#[derive(Debug)]
pub enum DepError {
    MissingDependency {
        service: String,
        dependency: String,
    },
    CycleDetected,
}

impl std::fmt::Display for DepError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDependency {
                service,
                dependency,
            } => {
                write!(f, "service '{service}' depends on unknown service '{dependency}'")
            }
            Self::CycleDetected => write!(f, "dependency cycle detected"),
        }
    }
}

impl std::error::Error for DepError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service(name: &str, deps: &[&str]) -> ServiceDef {
        ServiceDef {
            name: name.to_string(),
            binary: format!("/system/bin/{name}"),
            args: vec![],
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            restart: RestartPolicy::Always,
        }
    }

    #[test]
    fn simple_dependency_order() {
        let services = vec![
            make_service("cage", &["egress", "ctxgraph"]),
            make_service("egress", &[]),
            make_service("ctxgraph", &[]),
        ];

        let order = resolve_start_order(&services).unwrap();

        let cage_idx = order.iter().position(|s| s == "cage").unwrap();
        let egress_idx = order.iter().position(|s| s == "egress").unwrap();
        let ctx_idx = order.iter().position(|s| s == "ctxgraph").unwrap();

        assert!(egress_idx < cage_idx);
        assert!(ctx_idx < cage_idx);
    }

    #[test]
    fn no_dependencies() {
        let services = vec![
            make_service("a", &[]),
            make_service("b", &[]),
            make_service("c", &[]),
        ];

        let order = resolve_start_order(&services).unwrap();
        assert_eq!(order.len(), 3);
    }

    #[test]
    fn cycle_detected() {
        let services = vec![
            make_service("a", &["b"]),
            make_service("b", &["a"]),
        ];

        let result = resolve_start_order(&services);
        assert!(matches!(result, Err(DepError::CycleDetected)));
    }

    #[test]
    fn missing_dependency() {
        let services = vec![make_service("a", &["nonexistent"])];

        let result = resolve_start_order(&services);
        assert!(matches!(
            result,
            Err(DepError::MissingDependency { .. })
        ));
    }

    #[test]
    fn stop_order_is_reverse_of_start() {
        let services = vec![
            make_service("cage", &["egress"]),
            make_service("egress", &[]),
        ];

        let start = resolve_start_order(&services).unwrap();
        let stop = resolve_stop_order(&services).unwrap();

        assert_eq!(start[0], stop[1]);
        assert_eq!(start[1], stop[0]);
    }

    #[test]
    fn chain_dependency() {
        let services = vec![
            make_service("c", &["b"]),
            make_service("b", &["a"]),
            make_service("a", &[]),
        ];

        let order = resolve_start_order(&services).unwrap();
        assert_eq!(order, vec!["a", "b", "c"]);
    }
}
