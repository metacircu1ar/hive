//! Independent lenient fields for the dialog's role and login choices.
use super::Choice;
use crate::cli::views;
use serde::Deserialize;

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Reply<'a> {
    #[serde(borrow, deserialize_with = "views::rows")]
    pub roles: Vec<Row<'a>>,
    #[serde(borrow, deserialize_with = "views::rows")]
    pub profiles: Vec<Row<'a>>,
    #[serde(borrow, deserialize_with = "views::rows")]
    pub groups: Vec<Row<'a>>,
    #[serde(borrow, deserialize_with = "views::field")]
    pub default_provider: Option<&'a str>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
pub(super) struct Row<'a> {
    #[serde(borrow, deserialize_with = "views::field")]
    name: Option<&'a str>,
    #[serde(borrow, deserialize_with = "views::field")]
    provider: Option<&'a str>,
}

pub(super) fn collect(rows: Vec<Row<'_>>) -> Vec<Choice> {
    rows.into_iter()
        .filter_map(|row| {
            Some(Choice {
                name: row.name?.to_owned(),
                provider: row.provider.map(str::to_owned),
            })
        })
        .collect()
}
