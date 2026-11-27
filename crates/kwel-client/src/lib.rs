use kwel_native_host::{ipc, paths::RuntimePaths};
use kwel_protocol::{
    Code, Error, LocalRequest, LocalResponse, MAX_MESSAGE_BYTES, Message, Profile, Result,
    SearchRequestV2, VERSION, VERSION_2,
};
use std::{collections::BTreeMap, time::Duration};
#[derive(Clone)]
pub struct Client {
    pub paths: RuntimePaths,
}
impl Client {
    pub fn current() -> Result<Self> {
        Ok(Self {
            paths: RuntimePaths::current()?,
        })
    }
    pub async fn request(&self, request: LocalRequest) -> Result<LocalResponse> {
        request.validate()?;
        let message = request.message();
        let response = self.exchange(&request).await?;
        if message.v == VERSION_2 && response.version() == VERSION {
            if legacy_rejection(&response, &message.request_id) {
                return Err(Error(Code::IncompatibleVersion));
            }
            // An older Host cannot decode the new fields. Confirm its generic
            // decode error with a probe that v1 understands. Never replay the
            // original search or open operation.
            if (message.cursor.is_some() || message.include_snippet.is_some())
                && matches!(&response, LocalResponse::Error { request_id, code: Code::InvalidRequest, .. } if request_id.is_empty())
            {
                let request_id = uuid::Uuid::new_v4().to_string();
                let probe = LocalRequest::Probe {
                    v: VERSION_2,
                    request_id: request_id.clone(),
                    profile_id: message.profile_id.clone(),
                };
                if legacy_rejection(&self.exchange(&probe).await?, &request_id) {
                    return Err(Error(Code::IncompatibleVersion));
                }
            }
        }
        if response.version() != message.v || response.request_id() != message.request_id {
            return Err(Error(Code::InvalidRequest));
        }
        match (&request, &response) {
            (_, LocalResponse::Error { code, .. }) => return Err(Error(*code)),
            (
                LocalRequest::Probe { .. },
                LocalResponse::Profile {
                    profile_id,
                    profile_label,
                    host_version,
                    ..
                },
            ) if profile_id == &message.profile_id
                && kwel_protocol::units(profile_label) <= 64
                && !profile_label.contains(['\r', '\n'])
                && !host_version.is_empty()
                && host_version.len() <= 100 => {}
            (
                LocalRequest::Search { .. },
                LocalResponse::Search {
                    items, has_more, ..
                },
            ) if message.v == VERSION => Message {
                request_id: message.request_id.clone(),
                items: items.clone(),
                has_more: Some(*has_more),
                ..Message::new("search-result")
            }
            .response(&message.request_id, false, message.limit)?,
            (
                LocalRequest::Search { .. } | LocalRequest::SearchV2(_),
                LocalResponse::SearchV2(result),
            ) => {
                result.validate(&message.request_id, message.limit)?;
                if message.include_snippet != Some(true) && result.has_snippets() {
                    return Err(Error(Code::SearchFailed));
                }
            }
            (LocalRequest::Open { .. }, LocalResponse::Open { ok: true, .. }) => {}
            _ => return Err(Error(Code::InvalidRequest)),
        }
        Ok(response)
    }
    async fn exchange(&self, request: &LocalRequest) -> Result<LocalResponse> {
        let message = request.message();
        let mut stream = ipc::connect(&self.paths.endpoint(&message.profile_id, false)?).await?;
        let duration = if matches!(request, LocalRequest::Probe { .. }) {
            Duration::from_secs(4)
        } else {
            Duration::from_secs(15)
        };
        tokio::time::timeout(duration, async {
            ipc::write(&mut *stream, request).await?;
            let bytes = ipc::read(&mut *stream, MAX_MESSAGE_BYTES).await?;
            Ok(serde_json::from_slice(&bytes)?)
        })
        .await
        .map_err(|_| Error(Code::Timeout))?
    }
    pub async fn profiles(&self) -> Result<Vec<Profile>> {
        let ids = self.paths.profiles()?;
        let mut jobs = tokio::task::JoinSet::new();
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        for profile_id in ids.into_iter().take(64) {
            let client = self.clone();
            let semaphore = semaphore.clone();
            jobs.spawn(async move {
                let _permit = semaphore.acquire_owned().await.ok()?;
                match client
                    .request(LocalRequest::Probe {
                        v: VERSION,
                        request_id: uuid::Uuid::new_v4().to_string(),
                        profile_id,
                    })
                    .await
                    .ok()?
                {
                    LocalResponse::Profile {
                        profile_id,
                        profile_label,
                        host_version,
                        ..
                    } => Some(Profile {
                        profile_id,
                        profile_label,
                        host_version,
                    }),
                    _ => None,
                }
            });
        }
        let mut profiles = Vec::new();
        while let Some(result) = jobs.join_next().await {
            if let Ok(Some(profile)) = result {
                profiles.push(profile);
            }
        }
        profiles.sort_by(|a, b| a.profile_id.cmp(&b.profile_id));
        Ok(profiles)
    }
}

fn legacy_rejection(response: &LocalResponse, expected_id: &str) -> bool {
    matches!(
        response,
        LocalResponse::Error { v: VERSION, request_id, code: Code::IncompatibleVersion, .. }
            if request_id == expected_id
    )
}

pub enum Operation {
    Version,
    Profiles,
    Request(LocalRequest),
}
pub fn parse_args(arguments: &[String], format: &str) -> Result<Operation> {
    let operation = arguments.first().ok_or(Error(Code::InvalidRequest))?;
    if matches!(operation.as_str(), "version" | "--version") && arguments.len() == 1 {
        return Ok(Operation::Version);
    }
    let mut values = BTreeMap::new();
    let mut args = arguments[1..].iter();
    while let Some(key) = args.next() {
        let value = args.next().ok_or(Error(Code::InvalidRequest))?;
        if !key.starts_with("--") || values.insert(key.as_str(), value.as_str()).is_some() {
            return Err(Error(Code::InvalidRequest));
        }
    }
    if values
        .remove("--format")
        .is_some_and(|value| value != format)
    {
        return Err(Error(Code::InvalidRequest));
    }
    if operation == "profiles" {
        return if values.is_empty() {
            Ok(Operation::Profiles)
        } else {
            Err(Error(Code::InvalidRequest))
        };
    }
    let v = match values.remove("--protocol") {
        None => VERSION,
        Some("1") if format == "json" => VERSION,
        Some("2") if format == "json" => VERSION_2,
        _ => return Err(Error(Code::InvalidRequest)),
    };
    let profile_id = values
        .remove("--profile-id")
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let request_id = uuid::Uuid::new_v4().to_string();
    let request = match operation.as_str() {
        "profile-probe" => LocalRequest::Probe {
            v,
            request_id,
            profile_id,
        },
        "search" => {
            let query = values.remove("--query").unwrap_or("").to_string();
            let limit = values
                .remove("--limit")
                .unwrap_or("20")
                .parse()
                .map_err(|_| Error(Code::InvalidRequest))?;
            if v == VERSION_2 {
                let cursor = values.remove("--cursor").map(str::to_owned);
                let include_snippet = values
                    .remove("--include-snippet")
                    .map(str::parse)
                    .transpose()
                    .map_err(|_| Error(Code::InvalidRequest))?;
                LocalRequest::SearchV2(SearchRequestV2::new(
                    request_id,
                    profile_id,
                    query,
                    limit,
                    cursor,
                    include_snippet,
                ))
            } else {
                LocalRequest::Search {
                    v,
                    request_id,
                    profile_id,
                    query,
                    limit,
                }
            }
        }
        "open" | "open-page" => {
            let open_ref = values.remove("--ref").unwrap_or("").to_string();
            LocalRequest::Open {
                v,
                request_id,
                profile_id,
                open_ref,
            }
        }
        _ => return Err(Error(Code::InvalidRequest)),
    };
    if !values.is_empty() {
        return Err(Error(Code::InvalidRequest));
    }
    request.validate()?;
    Ok(Operation::Request(request))
}
