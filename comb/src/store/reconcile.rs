use super::{BTreeSet, Comb, ContentHash, Page, Result, Slug, Stats};

impl Comb {
    pub fn sync(&mut self) -> Result<Stats> {
        self.ready()?;
        tracing::info!(name: "comb.sync_started", phase = "scan");
        let mut present = BTreeSet::new();
        let mut externalized = 0_u64;
        self.disk
            .pages()
            .try_for_each(|item| -> Result<()> {
                let (slug, raw) = item?;
                present.insert(slug.clone());
                let raw = match raw {
                    Ok(raw) => raw,
                    Err(error) => {
                        self.index.quarantine(&slug, error.code.as_str())?;
                        return Ok(());
                    }
                };
                let hash = ContentHash::digest(&raw);
                let indexed = self.index.content_hash(&slug)?;
                let saved = self.disk.head(&slug)?;
                if indexed.as_ref() == Some(&hash) && saved.as_ref() == Some(&hash) {
                    return Ok(());
                }
                let page = match Page::parse(&raw) {
                    Ok(page) => page,
                    Err(error) => {
                        self.index.quarantine(&slug, error.code.as_str())?;
                        return Ok(());
                    }
                };
                let mut loaded = self.disk.load_parsed(&slug, raw, page)?;
                if loaded.trusted {
                    self.index.upsert(&slug, &loaded.raw, &loaded.page)?;
                } else {
                    self.disk.seal(&slug, &mut loaded.page)?;
                    self.persist(&slug, &loaded.page, Some(&loaded.hash), None)?;
                    externalized += 1;
                }
                Ok(())
            })
            .map_err(|error| {
                if externalized > 0 {
                    error.committed()
                } else {
                    error
                }
            })?;
        // Operational traversal failures abort before deletion. Malformed pages
        // remain on disk, while their stale searchable rows are quarantined.
        let stats = self.finish_sync(&present, externalized)?;
        tracing::info!(name: "comb.sync_finished", count = present.len(), success = true);
        if externalized > 0 {
            tracing::info!(name: "comb.pages_externalized",
                 count = externalized,
                 decision = "discard_untrusted_receipts");
        }
        Ok(stats)
    }

    fn finish_sync(&mut self, present: &BTreeSet<Slug>, externalized: u64) -> Result<Stats> {
        let result = (|| {
            self.index.remove_absent(present)?;
            self.stats()
        })();
        result.map_err(|error| {
            if externalized > 0 {
                error.committed()
            } else {
                error
            }
        })
    }
    /// Reconcile just a mutation's target, so a service get/conflict/get retry
    /// cannot keep returning an obsolete indexed hash after a hand edit.
    pub(crate) fn reconcile(&mut self, slug: &Slug) -> Result<()> {
        self.ready()?;
        let indexed = self.index.content_hash(slug)?;
        match self.disk.content(slug)? {
            Some(raw) if indexed.as_ref() != Some(&ContentHash::digest(&raw)) => {
                let page = Page::parse(&raw)?;
                let mut loaded = self.disk.load_parsed(slug, raw, page)?;
                if loaded.trusted {
                    self.index.upsert(slug, &loaded.raw, &loaded.page)?;
                } else {
                    self.disk.seal(slug, &mut loaded.page)?;
                    self.persist(slug, &loaded.page, Some(&loaded.hash), None)?;
                    tracing::debug!(name: "comb.page_externalized",
                         decision = "discard_untrusted_receipts");
                }
            }
            None if indexed.is_some() => self.index.delete(slug)?,
            _ => {}
        }
        Ok(())
    }
}
