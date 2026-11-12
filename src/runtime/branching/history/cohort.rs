//! Restricted ancestry proof for a native message's parallel tool results.
use super::{Kind, Result, refused};
use crate::decode::{field, object_or_default};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub(super) struct Cohort {
    current: Option<Group>,
}

struct Group {
    native: String,
    accepting_blocks: bool,
    members: BTreeMap<String, BTreeSet<String>>,
    declared: BTreeSet<String>,
    consumed: BTreeSet<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Projection<'a> {
    #[serde(rename = "sourceToolAssistantUUID", borrow, deserialize_with = "field")]
    source: Option<&'a str>,
    #[serde(borrow, deserialize_with = "object_or_default")]
    message: Message<'a>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Message<'a> {
    #[serde(borrow, deserialize_with = "field")]
    id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    content: Vec<Block<'a>>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Block<'a> {
    #[serde(rename = "type", borrow, deserialize_with = "field")]
    kind: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    id: Option<&'a str>,
    #[serde(borrow, deserialize_with = "field")]
    tool_use_id: Option<&'a str>,
}

impl Cohort {
    pub(super) fn observe(
        &mut self,
        value: &Value,
        kind: Kind,
        id: &str,
        parent: Option<&str>,
    ) -> Result<bool> {
        let view: Projection<'_> =
            crate::decode::object(value).map_err(|cause| refused().with_source(cause))?;
        if kind == Kind::Assistant {
            self.assistant(id, view.message)?;
            return Ok(false);
        }
        if let Some(group) = &mut self.current
            && group.result(view, parent)?
        {
            return Ok(true);
        }
        // Ordinary user input (including non-cohort serial results) ends this
        // exception. It still needs the existing rooted linear ancestry proof.
        self.current = None;
        Ok(false)
    }

    fn assistant(&mut self, id: &str, message: Message<'_>) -> Result<()> {
        let Some(native) = message.id.filter(|id| !id.is_empty()) else {
            self.current = None;
            return Ok(());
        };
        let group = self.current.get_or_insert_with(|| Group::new(native));
        if group.native != native || !group.accepting_blocks {
            *group = Group::new(native);
        }
        let mut tools = BTreeSet::new();
        for block in message.content {
            if block.kind == Some("tool_use") {
                let tool = block.id.filter(|id| !id.is_empty()).ok_or_else(refused)?;
                if !group.declared.insert(tool.to_owned()) {
                    return Err(refused());
                }
                tools.insert(tool.to_owned());
            }
        }
        group.members.insert(id.to_owned(), tools);
        Ok(())
    }
}

impl Group {
    fn new(native: &str) -> Self {
        Self {
            native: native.into(),
            accepting_blocks: true,
            members: BTreeMap::new(),
            declared: BTreeSet::new(),
            consumed: BTreeSet::new(),
        }
    }

    fn result(&mut self, view: Projection<'_>, parent: Option<&str>) -> Result<bool> {
        let Some(source) = view.source else {
            return Ok(false);
        };
        if parent != Some(source) || view.message.content.is_empty() {
            return Ok(false);
        }
        let Some(declared) = self.members.get(source) else {
            return Ok(false);
        };
        let mut pending = BTreeSet::new();
        for block in view.message.content {
            if block.kind != Some("tool_result") {
                return Ok(false);
            }
            let Some(tool) = block.tool_use_id else {
                return Ok(false);
            };
            if !declared.contains(tool) {
                return Ok(false);
            }
            if self.consumed.contains(tool) || !pending.insert(tool) {
                return Err(refused());
            }
        }
        self.consumed.extend(pending.into_iter().map(str::to_owned));
        self.accepting_blocks = false;
        Ok(true)
    }
}
