/// Repository-level clean-horizon outcomes plus shutdown-wide failures.
/// A pending repository is never mistaken for one whose checkpoint succeeded.
#[derive(Clone, Debug, Default)]
pub struct ShutdownReport {
    repositories: Vec<(String, Option<Result<(), String>>)>,
    errors: Vec<String>,
}

impl ShutdownReport {
    pub fn pending(repositories: Vec<String>) -> Self {
        Self {
            repositories: repositories.into_iter().map(|name| (name, None)).collect(),
            errors: Vec::new(),
        }
    }

    pub fn record(&mut self, repository: &str, result: Result<(), String>) {
        if let Some((_, outcome)) = self
            .repositories
            .iter_mut()
            .find(|(name, _)| name == repository)
        {
            *outcome = Some(result);
        } else {
            self.repositories.push((repository.into(), Some(result)));
        }
    }

    pub fn add_error(&mut self, error: impl Into<String>) {
        self.errors.push(error.into());
    }

    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Borrows repository names and error text from this report.
    pub fn repository_failures(&self) -> Vec<(&str, &str)> {
        self.repositories
            .iter()
            .filter_map(|(name, outcome)| match outcome {
                Some(Ok(())) => None,
                Some(Err(error)) => Some((name.as_str(), error.as_str())),
                None => Some((name.as_str(), "shutdown did not complete")),
            })
            .collect()
    }

    pub fn is_clean(&self) -> bool {
        self.errors.is_empty()
            && self
                .repositories
                .iter()
                .all(|(_, outcome)| matches!(outcome, Some(Ok(()))))
    }

    /// Supervisor contract: 70 means shutdown did not complete cleanly.
    pub fn exit_code(&self) -> u8 {
        if self.is_clean() {
            0
        } else {
            70
        }
    }
}
