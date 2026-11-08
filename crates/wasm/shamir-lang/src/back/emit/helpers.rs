use std::collections::BTreeSet;
use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct HelperId(pub u32);

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HelperOrderError {
    #[error("helper dependency references an unknown node")]
    UnknownDependency,
    #[error("helper dependency graph contains a cycle")]
    Cycle,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HelperGraph {
    dependencies: Vec<Vec<HelperId>>,
}

impl HelperGraph {
    pub fn add(&mut self, dependencies: &[HelperId]) -> HelperId {
        let id = HelperId(u32::try_from(self.dependencies.len()).expect("helper count fits u32"));
        self.dependencies.push(dependencies.to_vec());
        id
    }

    pub fn dependencies(&self, id: HelperId) -> Option<&[HelperId]> {
        self.dependencies.get(id.0 as usize).map(Vec::as_slice)
    }

    #[cfg(test)]
    pub fn add_dependency(&mut self, id: HelperId, dependency: HelperId) {
        self.dependencies[id.0 as usize].push(dependency);
    }

    pub fn order(&self, roots: &[HelperId]) -> Result<Vec<HelperId>, HelperOrderError> {
        let mut selected = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(id) = pending.pop() {
            let Some(dependencies) = self.dependencies(id) else {
                return Err(HelperOrderError::UnknownDependency);
            };
            if selected.insert(id) {
                pending.extend(dependencies.iter().copied());
            }
        }
        let mut indegree = vec![0usize; self.dependencies.len()];
        let mut dependents = vec![Vec::new(); self.dependencies.len()];
        for id in selected.iter().copied() {
            for dependency in self
                .dependencies(id)
                .ok_or(HelperOrderError::UnknownDependency)?
            {
                if selected.contains(dependency) {
                    indegree[id.0 as usize] += 1;
                    dependents[dependency.0 as usize].push(id);
                }
            }
        }
        let mut ready: BTreeSet<HelperId> = selected
            .iter()
            .copied()
            .filter(|id| indegree[id.0 as usize] == 0)
            .collect();
        let mut result = Vec::with_capacity(selected.len());
        while let Some(id) = ready.pop_first() {
            result.push(id);
            for dependent in &dependents[id.0 as usize] {
                indegree[dependent.0 as usize] -= 1;
                if indegree[dependent.0 as usize] == 0 {
                    ready.insert(*dependent);
                }
            }
        }
        if result.len() != selected.len() {
            return Err(HelperOrderError::Cycle);
        }
        Ok(result)
    }
}

pub(crate) fn emit_helper_order(
    graph: &HelperGraph,
    roots: &[HelperId],
) -> Result<Vec<HelperId>, HelperOrderError> {
    graph.order(roots)
}
