use crate::{
    Code, Error, Item, MAX_REQUEST_ID_UNITS, MAX_RESULTS, Result, VERSION_2, deserialize_version,
    units, valid_result_item,
};
use serde::{Deserialize, Deserializer, Serialize};

pub const MAX_CURSOR_UNITS: usize = 2_000;
pub const MAX_SNIPPET_SCALARS: usize = 160;

// A missing optional field uses its default. An explicitly null field is invalid.
pub(super) fn optional<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SearchType {
    Search,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchRequestV2 {
    #[serde(deserialize_with = "deserialize_version::<_, VERSION_2>")]
    v: u32,
    #[serde(rename = "type")]
    kind: SearchType,
    pub request_id: String,
    pub profile_id: String,
    pub query: String,
    pub limit: u32,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor: Option<String>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub include_snippet: Option<bool>,
}
impl SearchRequestV2 {
    pub fn new(
        request_id: String,
        profile_id: String,
        query: String,
        limit: u32,
        cursor: Option<String>,
        include_snippet: Option<bool>,
    ) -> Self {
        Self {
            v: VERSION_2,
            kind: SearchType::Search,
            request_id,
            profile_id,
            query,
            limit,
            cursor,
            include_snippet,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MatchField {
    Title,
    Url,
    Bookmark,
    Body,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SearchSource {
    History,
    Bookmark,
    Body,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchItemV2 {
    pub title: String,
    pub url: String,
    pub domain: String,
    pub open_ref: String,
    pub matched_in: Vec<MatchField>,
    #[serde(
        default,
        deserialize_with = "optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub snippet: Option<String>,
}
impl From<SearchItemV2> for Item {
    fn from(item: SearchItemV2) -> Self {
        Self {
            title: item.title,
            url: item.url,
            domain: item.domain,
            open_ref: item.open_ref,
        }
    }
}

fn unique<T: PartialEq>(values: &[T], maximum: usize) -> bool {
    values.len() <= maximum
        && values
            .iter()
            .enumerate()
            .all(|(i, value)| !values[..i].contains(value))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum SearchResultType {
    SearchResult,
}

/// The Native and Local v2 result shapes are identical; the Host replaces the request ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SearchResultV2 {
    #[serde(deserialize_with = "deserialize_version::<_, VERSION_2>")]
    v: u32,
    #[serde(rename = "type")]
    kind: SearchResultType,
    pub request_id: String,
    pub items: Vec<SearchItemV2>,
    #[serde(deserialize_with = "Deserialize::deserialize")]
    pub next_cursor: Option<String>,
    pub partial_sources: Vec<SearchSource>,
}
impl SearchResultV2 {
    pub fn validate(&self, expected_id: &str, limit: u32) -> Result<()> {
        if self.request_id.is_empty()
            || self.request_id != expected_id
            || units(&self.request_id) > MAX_REQUEST_ID_UNITS
        {
            return Err(Error(Code::InvalidRequest));
        }
        if self.items.len() > limit.min(MAX_RESULTS) as usize
            || self
                .next_cursor
                .as_ref()
                .is_some_and(|cursor| cursor.is_empty() || units(cursor) > MAX_CURSOR_UNITS)
            || !unique(&self.partial_sources, 3)
            || self.items.iter().any(|item| {
                !valid_result_item(&item.title, &item.url, &item.open_ref)
                    || !unique(&item.matched_in, 4)
                    || item.snippet.as_ref().is_some_and(|snippet| {
                        !item.matched_in.contains(&MatchField::Body)
                            || snippet.chars().take(MAX_SNIPPET_SCALARS + 1).count()
                                > MAX_SNIPPET_SCALARS
                    })
            })
        {
            return Err(Error(Code::SearchFailed));
        }
        Ok(())
    }

    pub fn has_snippets(&self) -> bool {
        self.items.iter().any(|item| item.snippet.is_some())
    }
}
