//! Immutable identity and checked capabilities shared by service and store.
use crate::{Actor, Error, Result, values::Principal};
use comb_protocol::service::api::actions::Capability;

pub(crate) struct Authority {
    actor: Actor,
    pub reviewer: bool,
    principal: Principal,
}
impl Authority {
    pub(crate) fn new(actor: Actor, reviewer: bool) -> Result<Self> {
        let principal = Principal::for_actor(&actor)?;
        Ok(Self {
            actor,
            reviewer,
            principal,
        })
    }
    pub(crate) fn principal(&self) -> &Principal {
        &self.principal
    }
    pub(crate) fn actor(&self) -> &Actor {
        &self.actor
    }
    pub fn human(&self) -> Result<()> {
        if self.actor.is_human() {
            Ok(())
        } else {
            Err(Error::forbidden())
        }
    }
}
pub(crate) struct Permission<'a> {
    authority: &'a Authority,
}
pub(crate) struct ReviewPermission<'a> {
    authority: &'a Authority,
}

impl Authority {
    pub(crate) fn collector_admission(&self, required: Capability) -> Result<()> {
        if matches!(self.actor(), Actor::Collector { .. }) && required != Capability::Capture {
            return Err(Error::collector_capture_only());
        }
        Ok(())
    }
    pub(crate) fn permit(&self, required: Capability) -> Result<Permission<'_>> {
        match required {
            Capability::Human => self.human()?,
            Capability::Review if !self.reviewer => {
                return Err(Error::configured_reviewer_required());
            }
            Capability::Read | Capability::Write | Capability::Capture | Capability::Review => {}
        }
        self.collector_admission(required)?;
        Ok(Permission { authority: self })
    }
    pub(crate) fn review_permission(&self) -> Result<ReviewPermission<'_>> {
        let permission = self.permit(Capability::Review)?;
        Ok(ReviewPermission {
            authority: permission.authority,
        })
    }
}
impl<'a> Permission<'a> {
    pub(crate) fn authority(self) -> &'a Authority {
        self.authority
    }
}
impl<'a> ReviewPermission<'a> {
    pub(crate) fn authority(self) -> &'a Authority {
        self.authority
    }
}

#[cfg(test)]
mod tests;
