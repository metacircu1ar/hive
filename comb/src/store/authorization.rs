//! Store write attribution extends checked identities without owning their grants.
use super::{ReviewedWrite, Write};
use crate::{
    Result,
    authorization::{Authority, Permission, ReviewPermission},
    values::RequestId,
};

impl Authority {
    pub(crate) fn write<'a>(
        &'a self,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> crate::store::Write<'a> {
        crate::store::Write::authorized(self, request_id, run)
    }
    pub(crate) fn reviewed_write<'a>(
        &'a self,
        request_id: &RequestId,
        run: Option<&'a str>,
    ) -> Result<ReviewedWrite<'a>> {
        Ok(ReviewedWrite::authorized(
            self.review_permission()?,
            request_id,
            run,
        ))
    }
}
impl<'a> Permission<'a> {
    pub(crate) fn write(self, request_id: &RequestId, run: Option<&'a str>) -> Write<'a> {
        self.authority().write(request_id, run)
    }
}
impl<'a> ReviewPermission<'a> {
    pub(crate) fn write(self, request_id: &RequestId, run: Option<&'a str>) -> Write<'a> {
        self.authority().write(request_id, run)
    }
}
