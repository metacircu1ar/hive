//! Complete non-health service observations shared by status and setup planning.
use super::{Controller, Health, Spec, Status, load, manager::State};
use crate::Result;

#[expect(
    clippy::struct_excessive_bools,
    reason = "Registration, manager availability and unit freshness are independent observations."
)]
pub struct Observation {
    pub managed: bool,
    pub outdated_unit: bool,
    pub manager_available: bool,
    pub registered: bool,
    pub enabled: Option<bool>,
    pub label: String,
    pub missing_environment: Vec<String>,
    pub failure: Option<crate::managed::Failure>,
    pub log: std::path::PathBuf,
    pub note: Option<String>,
    pub manual_start: String,
    pub service_path: Option<String>,
    pub warnings: Vec<String>,
}
impl Observation {
    pub fn with_readiness(self, ready: bool) -> Status {
        let Self {
            managed,
            outdated_unit,
            manager_available,
            registered,
            enabled,
            label,
            missing_environment,
            failure,
            log,
            note,
            manual_start,
            service_path,
            warnings,
        } = self;
        Status {
            managed,
            outdated_unit,
            manager_available,
            registered,
            enabled,
            ready,
            label,
            missing_environment,
            failure,
            log,
            note,
            manual_start,
            service_path,
            warnings,
        }
    }
}
impl Controller<'_> {
    pub fn status(&self, health: &Health<'_>) -> Result<Status> {
        let observation = self.inspect()?;
        Ok(observation.with_readiness(health()?))
    }
    pub fn inspect(&self) -> Result<Observation> {
        let spec = load(self.layout, self.scope)?;
        let manager = self.manager();
        let available = manager.available()?;
        Ok(Observation {
            manual_start: self.scope.foreground_command()?,
            service_path: spec
                .as_ref()
                .and_then(|s| s.environment.get("PATH").cloned()),
            warnings: vec![],
            managed: spec.is_some(),
            outdated_unit: spec
                .as_ref()
                .map(|s| Ok::<_, std::io::Error>(s.unit_contents.as_bytes() != s.render()?))
                .transpose()?
                .unwrap_or(false),
            enabled: if available && spec.is_some() {
                manager.enabled(self.scope)?
            } else {
                None
            },
            manager_available: available,
            registered: if available && spec.is_some() {
                manager.state(self.scope)? != State::Absent
            } else {
                false
            },
            label: self.scope.label()?,
            missing_environment: spec
                .as_ref()
                .map(Spec::missing_environment)
                .transpose()?
                .unwrap_or_default(),
            failure: spec
                .as_ref()
                .map(crate::managed::failure)
                .transpose()?
                .flatten(),
            log: self.layout.log(self.scope)?,
            note: if available {
                manager.login_note()?
            } else {
                Some(
                    "Not managed: no user service manager. Run the daemon in the foreground."
                        .into(),
                )
            },
        })
    }
}
