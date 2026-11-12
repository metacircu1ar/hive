//! Exact internal tool identities and special argument decisions.
use crate::enums::{domain_enum, wire_enum};

domain_enum!(
    #[derive(Ord, PartialOrd)]
    Tool {
        Spawn => "hive_spawn",
        Send => "hive_send",
        Batch => "hive_batch",
        SetAuth => "hive_set_auth",
        SetName => "hive_set_name",
        OptionsCatalog => "hive_options_catalog",
        SetOptions => "hive_set_options",
        Ask => "hive_ask",
        Complete => "hive_complete",
        Terminate => "hive_terminate",
        Agents => "hive_agents",
        Read => "hive_read",
        Messages => "hive_messages",
        Status => "hive_status",
        #[cfg(feature = "comb")]
        Evidence => "hive_evidence",
    }
);
wire_enum!(Argument {
    Operation => "op",
    RequestId => "request_id",
    Schema => "schema",
    Options => "options",
    Summary => "summary",
    Commit => "commit",
    Message => "message",
    Title => "title",
    Instructions => "instructions",
    Operations => "operations",
    Attachments => "attachments",
    Files => "files",
    Join => "join",
    StalledAfter => "stalled_after",
});
#[derive(Clone, Copy)]
pub(super) struct Field<'a> {
    pub name: &'a str,
    pub kind: Argument,
}
impl<'a> Field<'a> {
    pub fn new(name: &'a str) -> Self {
        Self {
            name,
            kind: Argument::parse(name),
        }
    }
}
impl std::fmt::Display for Field<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name)
    }
}
