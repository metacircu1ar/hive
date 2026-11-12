//! Restore every admitted service; one failure must not strand later scopes.
use super::{
    Held, Operation, Path, Probe, Recovery, Result, Runner, Saved, Services, Spec, publish,
};

pub(super) fn present(ownership: &Recovery) -> Result<&Held> {
    match ownership {
        Recovery::Present(held) => Ok(held),
        Recovery::Removed(_) => Err("Removed recovery scope cannot enter a new transaction".into()),
    }
}
pub(super) fn present_mut(ownership: &mut Recovery) -> Result<&mut Held> {
    match ownership {
        Recovery::Present(held) => Ok(held),
        Recovery::Removed(_) => Err("Removed recovery scope cannot enter a new transaction".into()),
    }
}

impl Services {
    pub(super) fn restore_all(&mut self, published: bool) -> Result<()> {
        let journal = self.journal.as_ref().ok_or("Missing service transaction")?;
        if self.held.len() != journal.services.len() {
            return Err("Service recovery ownership does not cover the saved scopes".into());
        }
        let pointer = publish::current(&journal.change.manager)?;
        let installed = publish::load(
            &journal.change.manager,
            pointer.as_deref(),
            &crate::manifest::native_target()?,
        )?;
        let commands = publish::links(&installed.parts)?;
        let restoration = Restoration {
            operation: journal.change.operation,
            prefix: &journal.change.prefix,
            commands: &commands,
            published,
            runner: self.runner.as_ref(),
            health: self.health.as_ref(),
        };
        let mut result = Ok(());
        for (ownership, saved) in self.held.iter_mut().zip(&journal.services) {
            let Recovery::Present(held) = ownership else {
                continue;
            };
            let restored = restoration.restore(held, saved);
            result = crate::lifecycle::settle(result, restored);
        }
        result
    }
}

struct Restoration<'a> {
    operation: Operation,
    prefix: &'a Path,
    commands: &'a std::collections::BTreeMap<std::path::PathBuf, std::path::PathBuf>,
    published: bool,
    runner: &'a dyn Runner,
    health: &'a Probe,
}
impl Restoration<'_> {
    fn restore(&self, held: &mut Held, saved: &Saved) -> Result<()> {
        let ready = || {
            self.check_commands(&saved.spec)
                .map_err(crate::Error::into_io_other)?;
            (self.health)(&saved.spec)
                .map(|p| p.ready)
                .map_err(crate::Error::into_io_other)
        };
        if self.operation == Operation::Uninstall && self.published {
            held.uninstall(self.runner, &ready)?;
            return Ok(());
        }
        self.check_commands(&saved.spec)?;
        if self.operation == Operation::Uninstall {
            held.install(self.runner)?;
        } else {
            held.restore_unit(self.runner, &saved.spec, &saved.next, self.published)?;
        }
        if saved.loaded {
            held.start(self.runner, &ready)?;
        }
        Ok(())
    }
    fn check_commands(&self, spec: &Spec) -> Result<()> {
        let mut names = vec![spec.program.strip_prefix(self.prefix)?.to_owned()];
        if spec.scope.product == "hive" {
            names.push("bin/hivectl".into());
        }
        let expected = names
            .into_iter()
            .map(|name| {
                let target = self
                    .commands
                    .get(&name)
                    .ok_or("Missing owned service command")?;
                Ok((name, target.clone()))
            })
            .collect::<Result<_>>()?;
        if !publish::check_links(self.prefix, &expected)?.is_empty() {
            return Err("Service command links need recovery before service restoration".into());
        }
        Ok(())
    }
}
