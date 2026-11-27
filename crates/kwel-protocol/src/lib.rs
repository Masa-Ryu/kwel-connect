//! Wire definitions shared by the Native Messaging and public local protocols.
//! Neither protocol depends on Alfred. See docs/ for language-neutral schemas.
use serde::{Deserialize, Deserializer, Serialize};
use std::{
    fmt,
    io::{Read, Write},
};

pub const VERSION: u32 = 1;
pub const VERSION_2: u32 = 2;
pub const MAX_MESSAGE_BYTES: usize = 1_000_000;
pub const MAX_LOCAL_REQUEST_BYTES: usize = 16_384;
pub const MAX_QUERY_UNITS: usize = 200;
pub const MAX_RESULTS: u32 = 100;
pub const MAX_REQUEST_ID_UNITS: usize = 100;
pub const MAX_PROFILE_LABEL_UNITS: usize = 64;
pub const HOST_NAME: &str = "com.kwel.bridge";
pub const EXTENSION_ID: &str = env!("KWEL_EXTENSION_ID");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Code {
    BadRequest,
    InvalidRequest,
    IncompatibleVersion,
    NotReady,
    ResultExpired,
    OpenFailed,
    SearchFailed,
    ProfileConflict,
    HostNotFound,
    Disconnected,
    Timeout,
    PermissionRequired,
    Busy,
    UnsafePath,
    InstallConflict,
    DowngradeRefused,
    HostInUse,
}
impl Code {
    pub fn message(self) -> &'static str {
        match self {
            Self::BadRequest | Self::InvalidRequest => "The request is invalid.",
            Self::IncompatibleVersion => {
                "The installed Host uses an incompatible protocol version."
            }
            Self::NotReady => "Kwel is not ready.",
            Self::ResultExpired => "This Kwel result has expired. Search again.",
            Self::OpenFailed => "Could not open this result. Search again.",
            Self::SearchFailed => "Kwel could not complete this search.",
            Self::ProfileConflict => {
                "This Kwel profile is connected more than once. Search stopped to avoid mixing profiles."
            }
            Self::HostNotFound | Self::Disconnected => {
                "The configured Kwel profile is not connected. Check Local Connection in that Chrome profile."
            }
            Self::Timeout => "Kwel took too long to respond. Try again.",
            Self::PermissionRequired => "Enable Local Connection in Kwel Settings.",
            Self::Busy => "Kwel is handling too many requests. Try again.",
            Self::UnsafePath => "Kwel refused an unsafe filesystem path or permission.",
            Self::InstallConflict => "An unrecognized Native Host registration already exists.",
            Self::DowngradeRefused => "A newer compatible Kwel Host is already installed.",
            Self::HostInUse => "Kwel Host is in use. Disable Local Connection before removing it.",
        }
    }
    pub fn native(self) -> bool {
        !matches!(
            self,
            Self::Busy
                | Self::UnsafePath
                | Self::InstallConflict
                | Self::DowngradeRefused
                | Self::HostInUse
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Error(pub Code);
pub type Result<T> = std::result::Result<T, Error>;
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.message())
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        use std::io::ErrorKind;
        Self(match error.kind() {
            ErrorKind::TimedOut => Code::Timeout,
            ErrorKind::PermissionDenied => Code::UnsafePath,
            ErrorKind::WouldBlock | ErrorKind::AddrInUse | ErrorKind::AlreadyExists => Code::Busy,
            _ => Code::Disconnected,
        })
    }
}
impl From<serde_json::Error> for Error {
    fn from(_: serde_json::Error) -> Self {
        Self(Code::InvalidRequest)
    }
}
pub fn id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if [8, 13, 18, 23].contains(&i) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
        && matches!(value.as_bytes()[14], b'1'..=b'8')
        && matches!(
            value.as_bytes()[19],
            b'8' | b'9' | b'a' | b'b' | b'A' | b'B'
        )
}
pub fn units(value: &str) -> usize {
    value.encode_utf16().count()
}
fn deserialize_version<'de, D: Deserializer<'de>, const VERSION: u32>(
    deserializer: D,
) -> std::result::Result<u32, D::Error> {
    let version = u32::deserialize(deserializer)?;
    if version == VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(
            "Unexpected version for this message shape",
        ))
    }
}
pub fn valid_origin(value: &str, extension: &str) -> bool {
    extension.len() == 32
        && extension.bytes().all(|b| (b'a'..=b'p').contains(&b))
        && (value == format!("chrome-extension://{extension}/")
            || value == format!("chrome-extension://{extension}"))
}
mod uri;
pub fn http_url(value: &str) -> bool {
    uri::http(value)
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Item {
    pub title: String,
    pub url: String,
    pub domain: String,
    pub open_ref: String,
}
fn valid_result_item(title: &str, url: &str, open_ref: &str) -> bool {
    !title.trim().is_empty() && http_url(url) && id(open_ref)
}

mod search_v2;
pub use search_v2::{
    MAX_CURSOR_UNITS, MAX_SNIPPET_SCALARS, MatchField, SearchItemV2, SearchRequestV2,
    SearchResultV2, SearchSource,
};

// Internal representation. Native messages and Local requests each select
// their own wire fields. Host coordination uses private Control IPC.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Message {
    pub v: u32,
    pub kind: String,
    pub request_id: String,
    pub query: String,
    pub limit: u32,
    pub cursor: Option<String>,
    pub include_snippet: Option<bool>,
    pub profile_id: String,
    pub profile_label: String,
    pub open_ref: String,
    pub items: Vec<Item>,
    pub has_more: Option<bool>,
    pub search_result_v2: Option<SearchResultV2>,
    pub code: Option<Code>,
    pub text: String,
    pub ok: bool,
}
mod native;
impl Message {
    pub fn native(&self) -> Result<()> {
        native::validate(self)
    }
    pub fn new(kind: &str) -> Self {
        Self {
            v: VERSION,
            kind: kind.into(),
            ..Self::default()
        }
    }
    pub fn error(request_id: &str, code: Code) -> Self {
        Self {
            request_id: request_id.into(),
            code: Some(code),
            text: code.message().into(),
            ..Self::new("error")
        }
    }
    pub fn envelope(&self) -> Result<()> {
        if !matches!(self.v, VERSION | VERSION_2) {
            return Err(Error(Code::IncompatibleVersion));
        }
        if self.kind.is_empty() {
            return Err(Error(Code::InvalidRequest));
        }
        Ok(())
    }
    pub fn hello(&self) -> Result<()> {
        self.envelope()?;
        if self.kind != "hello"
            || !id(&self.profile_id)
            || units(&self.profile_label) > MAX_PROFILE_LABEL_UNITS
            || self.profile_label.contains(['\r', '\n'])
        {
            return Err(Error(Code::InvalidRequest));
        }
        Ok(())
    }
    pub fn search(&self) -> Result<()> {
        self.envelope()?;
        if self.kind != "search"
            || !self.profile_id.is_empty()
            || self.request_id.is_empty()
            || units(&self.request_id) > MAX_REQUEST_ID_UNITS
            || units(&self.query) > MAX_QUERY_UNITS
            || !(1..=MAX_RESULTS).contains(&self.limit)
            || (self.v == VERSION && (self.cursor.is_some() || self.include_snippet.is_some()))
            || self
                .cursor
                .as_ref()
                .is_some_and(|cursor| units(cursor) > MAX_CURSOR_UNITS)
        {
            return Err(Error(Code::InvalidRequest));
        }
        Ok(())
    }
    pub fn open(&self) -> Result<()> {
        self.envelope()?;
        if self.kind != "open-page"
            || !self.profile_id.is_empty()
            || self.request_id.is_empty()
            || units(&self.request_id) > MAX_REQUEST_ID_UNITS
            || self.open_ref.is_empty()
            || units(&self.open_ref) > MAX_REQUEST_ID_UNITS
        {
            return Err(Error(Code::InvalidRequest));
        }
        Ok(())
    }
    pub fn response(&self, expected_id: &str, opening: bool, limit: u32) -> Result<()> {
        self.envelope()?;
        if self.request_id.is_empty()
            || units(&self.request_id) > MAX_REQUEST_ID_UNITS
            || expected_id.is_empty()
            || self.request_id != expected_id
        {
            return Err(Error(Code::InvalidRequest));
        }
        if self.kind == "error" {
            return if self.code.is_some_and(Code::native) {
                Ok(())
            } else {
                Err(Error(Code::InvalidRequest))
            };
        }
        if opening {
            if self.kind != "open-result" || !self.ok {
                return Err(Error(Code::OpenFailed));
            }
        } else {
            if self.kind != "search-result" {
                return Err(Error(Code::SearchFailed));
            }
            if self.v == VERSION_2 {
                let result = self
                    .search_result_v2
                    .as_ref()
                    .ok_or(Error(Code::SearchFailed))?;
                result.validate(expected_id, limit)?;
            } else if self.has_more.is_none()
                || self.items.len() > limit.min(MAX_RESULTS) as usize
                || self
                    .items
                    .iter()
                    .any(|item| !valid_result_item(&item.title, &item.url, &item.open_ref))
            {
                return Err(Error(Code::SearchFailed));
            }
        }
        Ok(())
    }
}
/// Decode the existing v1 contract, including its version rejection rules.
pub fn decode(payload: &[u8]) -> Result<Message> {
    let message = decode_native(payload)?;
    if message.v != VERSION {
        return Err(Error(Code::IncompatibleVersion));
    }
    Ok(message)
}

/// Decode either Native version. The transport checks the version bound at hello.
pub fn decode_native(payload: &[u8]) -> Result<Message> {
    if payload.is_empty() || payload.len() > MAX_MESSAGE_BYTES {
        return Err(Error(Code::InvalidRequest));
    }
    let message: Message = serde_json::from_slice(payload)?;
    message.native()?;
    Ok(message)
}
pub fn frame_length(header: [u8; 4], maximum: usize) -> Result<usize> {
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > maximum {
        return Err(Error(Code::InvalidRequest));
    }
    Ok(length)
}
pub fn read_frame(reader: &mut impl Read, maximum: usize) -> Result<Vec<u8>> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let mut data = vec![0; frame_length(header, maximum)?];
    reader.read_exact(&mut data)?;
    Ok(data)
}
pub fn encode_frame(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>> {
    let data = serde_json::to_vec(value)?;
    if data.is_empty() || data.len() > maximum {
        return Err(Error(Code::InvalidRequest));
    }
    let mut frame = (data.len() as u32).to_le_bytes().to_vec();
    frame.extend(data);
    Ok(frame)
}
pub fn write_frame(writer: &mut impl Write, value: &impl Serialize, maximum: usize) -> Result<()> {
    writer.write_all(&encode_frame(value, maximum)?)?;
    writer.flush()?;
    Ok(())
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum LocalRequest {
    #[serde(rename = "profile-probe")]
    Probe {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "profileId")]
        profile_id: String,
    },
    #[serde(rename = "search")]
    Search {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "profileId")]
        profile_id: String,
        query: String,
        limit: u32,
    },
    #[serde(rename = "open-page")]
    Open {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "profileId")]
        profile_id: String,
        #[serde(rename = "openRef")]
        open_ref: String,
    },
    #[serde(untagged)]
    SearchV2(SearchRequestV2),
}
impl LocalRequest {
    pub fn validate(&self) -> Result<()> {
        let mut message = self.message();
        message.envelope()?;
        if !id(&message.profile_id)
            || message.request_id.is_empty()
            || units(&message.request_id) > MAX_REQUEST_ID_UNITS
        {
            return Err(Error(Code::InvalidRequest));
        }
        // Public clients must select a profile and can only open UUID references.
        // Native messages have neither this routing field nor this client policy.
        if matches!(self, Self::Open { .. }) && !id(&message.open_ref) {
            return Err(Error(Code::InvalidRequest));
        }
        message.profile_id.clear();
        match self {
            Self::Search { .. } | Self::SearchV2(_) => message.search(),
            Self::Open { .. } => message.open(),
            Self::Probe { .. } => Ok(()),
        }
    }
    pub fn message(&self) -> Message {
        match self {
            Self::Probe {
                v,
                request_id,
                profile_id,
            } => Message {
                v: *v,
                request_id: request_id.clone(),
                profile_id: profile_id.to_lowercase(),
                ..Message::new("profile-probe")
            },
            Self::Search {
                v,
                request_id,
                profile_id,
                query,
                limit,
            } => Message {
                v: *v,
                request_id: request_id.clone(),
                profile_id: profile_id.to_lowercase(),
                query: query.clone(),
                limit: *limit,
                ..Message::new("search")
            },
            Self::Open {
                v,
                request_id,
                profile_id,
                open_ref,
            } => Message {
                v: *v,
                request_id: request_id.clone(),
                profile_id: profile_id.to_lowercase(),
                open_ref: open_ref.clone(),
                ..Message::new("open-page")
            },
            Self::SearchV2(search) => Message {
                v: VERSION_2,
                request_id: search.request_id.clone(),
                profile_id: search.profile_id.to_lowercase(),
                query: search.query.clone(),
                limit: search.limit,
                cursor: search.cursor.clone(),
                include_snippet: search.include_snippet,
                ..Message::new("search")
            },
        }
    }
    pub fn version(&self) -> u32 {
        match self {
            Self::Probe { v, .. } | Self::Search { v, .. } | Self::Open { v, .. } => *v,
            Self::SearchV2(_) => VERSION_2,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Profile {
    pub profile_id: String,
    pub profile_label: String,
    pub host_version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum LocalResponse {
    #[serde(rename = "profile-info")]
    Profile {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        #[serde(rename = "profileId")]
        profile_id: String,
        #[serde(rename = "profileLabel")]
        profile_label: String,
        #[serde(rename = "hostVersion")]
        host_version: String,
    },
    #[serde(rename = "search-result")]
    Search {
        #[serde(deserialize_with = "deserialize_version::<_, VERSION>")]
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        items: Vec<Item>,
        #[serde(rename = "hasMore")]
        has_more: bool,
    },
    #[serde(rename = "open-result")]
    Open {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        ok: bool,
    },
    #[serde(rename = "error")]
    Error {
        v: u32,
        #[serde(rename = "requestId")]
        request_id: String,
        code: Code,
        message: String,
    },
    #[serde(untagged)]
    SearchV2(SearchResultV2),
}
impl LocalResponse {
    pub fn profile(request_id: &str, profile: &Profile) -> Self {
        Self::profile_at_version(VERSION, request_id, profile)
    }
    pub fn profile_at_version(v: u32, request_id: &str, profile: &Profile) -> Self {
        Self::Profile {
            v,
            request_id: request_id.into(),
            profile_id: profile.profile_id.clone(),
            profile_label: profile.profile_label.clone(),
            host_version: profile.host_version.clone(),
        }
    }
    pub fn error(request_id: &str, code: Code) -> Self {
        Self::error_at_version(VERSION, request_id, code)
    }
    pub fn error_at_version(v: u32, request_id: &str, code: Code) -> Self {
        Self::Error {
            v,
            request_id: request_id.into(),
            code,
            message: code.message().into(),
        }
    }
    pub fn request_id(&self) -> &str {
        match self {
            Self::Profile { request_id, .. }
            | Self::Search { request_id, .. }
            | Self::Open { request_id, .. }
            | Self::Error { request_id, .. } => request_id,
            Self::SearchV2(result) => &result.request_id,
        }
    }
    pub fn version(&self) -> u32 {
        match self {
            Self::Profile { v, .. }
            | Self::Search { v, .. }
            | Self::Open { v, .. }
            | Self::Error { v, .. } => *v,
            Self::SearchV2(_) => VERSION_2,
        }
    }
}
