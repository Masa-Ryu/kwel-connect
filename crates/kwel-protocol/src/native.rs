//! Native-only fields. Public client routing and private migration operations
//! deliberately cannot be encoded/decoded through this channel.
use crate::search_v2::optional;
use crate::*;
use serde::{Deserializer, Serializer};

fn default_limit() -> u32 {
    20
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Wire {
    Hello {
        v: u32,
        profile_id: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        profile_label: String,
    },
    HelloAck {
        v: u32,
    },
    Ping {
        v: u32,
    },
    Pong {
        v: u32,
    },
    Search {
        v: u32,
        request_id: String,
        query: String,
        #[serde(default = "default_limit")]
        limit: u32,
        #[serde(
            default,
            deserialize_with = "optional",
            skip_serializing_if = "Option::is_none"
        )]
        cursor: Option<String>,
        #[serde(
            default,
            deserialize_with = "optional",
            skip_serializing_if = "Option::is_none"
        )]
        include_snippet: Option<bool>,
    },
    SearchResult {
        v: u32,
        request_id: String,
        items: Vec<Item>,
        has_more: bool,
    },
    OpenPage {
        v: u32,
        request_id: String,
        open_ref: String,
    },
    OpenResult {
        v: u32,
        request_id: String,
        ok: bool,
    },
    Error {
        v: u32,
        #[serde(default)]
        request_id: Option<String>,
        code: Code,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
    #[serde(untagged)]
    SearchResultV2(SearchResultV2),
}

pub(super) fn validate(message: &Message) -> Result<()> {
    Wire::try_from(message).map(|_| ())
}

impl From<Wire> for Message {
    fn from(wire: Wire) -> Self {
        match wire {
            Wire::Hello {
                v,
                profile_id,
                profile_label,
            } => Self {
                v,
                profile_id,
                profile_label,
                ..Self::new("hello")
            },
            Wire::HelloAck { v } => Self {
                v,
                ..Self::new("hello-ack")
            },
            Wire::Ping { v } => Self {
                v,
                ..Self::new("ping")
            },
            Wire::Pong { v } => Self {
                v,
                ..Self::new("pong")
            },
            Wire::Search {
                v,
                request_id,
                query,
                limit,
                cursor,
                include_snippet,
            } => Self {
                v,
                request_id,
                query,
                limit,
                cursor,
                include_snippet,
                ..Self::new("search")
            },
            Wire::SearchResult {
                v,
                request_id,
                items,
                has_more,
            } => Self {
                v,
                request_id,
                items,
                has_more: Some(has_more),
                ..Self::new("search-result")
            },
            Wire::SearchResultV2(result) => Self {
                v: VERSION_2,
                request_id: result.request_id.clone(),
                search_result_v2: Some(result),
                ..Self::new("search-result")
            },
            Wire::OpenPage {
                v,
                request_id,
                open_ref,
            } => Self {
                v,
                request_id,
                open_ref,
                ..Self::new("open-page")
            },
            Wire::OpenResult { v, request_id, ok } => Self {
                v,
                request_id,
                ok,
                ..Self::new("open-result")
            },
            Wire::Error {
                v,
                request_id,
                code,
                message,
            } => Self {
                v,
                request_id: request_id.unwrap_or_default(),
                code: Some(code),
                text: message,
                ..Self::new("error")
            },
        }
    }
}

impl TryFrom<&Message> for Wire {
    type Error = Error;
    fn try_from(message: &Message) -> Result<Self> {
        message.envelope()?;
        let v = message.v;
        let request_id = message.request_id.clone();
        let wire = match message.kind.as_str() {
            "hello" => {
                message.hello()?;
                Self::Hello {
                    v,
                    profile_id: message.profile_id.clone(),
                    profile_label: message.profile_label.clone(),
                }
            }
            "hello-ack" => Self::HelloAck { v },
            "ping" => Self::Ping { v },
            "pong" => Self::Pong { v },
            "search" => {
                message.search()?;
                Self::Search {
                    v,
                    request_id,
                    query: message.query.clone(),
                    limit: message.limit,
                    cursor: message.cursor.clone(),
                    include_snippet: message.include_snippet,
                }
            }
            "search-result" => {
                message.response(&request_id, false, MAX_RESULTS)?;
                if let Some(result) = &message.search_result_v2 {
                    Self::SearchResultV2(result.clone())
                } else {
                    Self::SearchResult {
                        v,
                        request_id,
                        items: message.items.clone(),
                        has_more: message.has_more.ok_or(Error(Code::InvalidRequest))?,
                    }
                }
            }
            "open-page" => {
                message.open()?;
                Self::OpenPage {
                    v,
                    request_id,
                    open_ref: message.open_ref.clone(),
                }
            }
            "open-result" => {
                message.response(&request_id, true, 0)?;
                Self::OpenResult {
                    v,
                    request_id,
                    ok: message.ok,
                }
            }
            "error" => {
                if units(&request_id) > MAX_REQUEST_ID_UNITS
                    || !message.code.is_some_and(Code::native)
                {
                    return Err(Error(Code::InvalidRequest));
                }
                Self::Error {
                    v,
                    request_id: (!request_id.is_empty()).then_some(request_id),
                    code: message.code.ok_or(Error(Code::InvalidRequest))?,
                    message: message.text.clone(),
                }
            }
            _ => return Err(Error(Code::InvalidRequest)),
        };
        // Do not silently drop routing, management or wrong-type fields when
        // encoding an internal request as a Native message.
        if Message::from(wire.clone()) != *message {
            return Err(Error(Code::InvalidRequest));
        }
        Ok(wire)
    }
}

impl Serialize for Message {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        Wire::try_from(self)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Message {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        if matches!(&wire,Wire::Error { request_id:Some(id),.. } if id.is_empty()) {
            return Err(serde::de::Error::custom("Invalid request ID"));
        }
        Ok(Message::from(wire))
    }
}
