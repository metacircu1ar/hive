//! Confirm pointer durability, recover each scope, then clear the journal last.
use super::{
    Controller, Journal, Lifecycle, Operation, PENDING, Path, Recovery, Result, Saved, Services,
    files, fs, links, publish, record, validate,
};

impl Services {
    pub(super) fn recover_pending(&mut self, prefix: &Path, manager: &Path) -> Result<()> {
        let Some(journal): Option<Journal> = record::read(&manager.join(PENDING))? else {
            return Ok(());
        };
        validate(&journal, prefix, manager)?;
        let actual = publish::current(manager)?;
        let previous = actual == journal.change.previous;
        if !previous && actual.as_ref() != Some(&journal.change.next) {
            return Err(
                "Pending installation does not match the published pointer; recovery \
                evidence retained"
                    .into(),
            );
        }
        // No service can run on a pointer whose publication barrier is uncertain.
        publish::sync(manager)?;
        self.held = journal
            .services
            .iter()
            .map(|saved| self.acquire_recovery(&journal, saved))
            .collect::<Result<Vec<_>>>()?;
        self.journal = Some(journal);
        let links = self.recover_links(prefix, manager, previous);
        let restored = self.restore(!previous);
        crate::lifecycle::settle(restored, links)?;
        self.clear()?;
        eprintln!("Recovered the pending installation and its original service running state.");
        Ok(())
    }
    fn acquire_recovery(&self, journal: &Journal, saved: &Saved) -> Result<Recovery> {
        let controller = Controller {
            layout: &self.layout,
            scope: &saved.spec.scope,
            runner: self.runner.as_ref(),
        };
        Ok(if journal.change.operation == Operation::Uninstall {
            // This transaction itself may have removed the descriptor before
            // pointer publication. Rollback must recreate its own removal.
            Recovery::Present(Box::new(controller.hold_expected(&saved.spec)?))
        } else {
            controller.recover_transition(&saved.spec, &saved.next)?
        })
    }
    fn recover_links(&self, prefix: &Path, manager: &Path, previous: bool) -> Result<()> {
        let journal = self.journal.as_ref().ok_or("Missing transaction")?;
        links::recover(prefix, manager, journal, previous)?;
        if previous {
            for (name, target) in &journal.change.added_links {
                let path = prefix.join(name);
                match fs::read_link(&path) {
                    Ok(current) if current == *target => {
                        fs::remove_file(&path)?;
                        files::sync_parent(&path)?;
                    }
                    Ok(_) => return Err("Interrupted installation command link was edited".into()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }
}
