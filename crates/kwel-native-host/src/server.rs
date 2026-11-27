use crate::{ipc, paths::RuntimePaths, secure};
use kwel_protocol::{
    Code, EXTENSION_ID, Error, LocalRequest, LocalResponse, MAX_LOCAL_REQUEST_BYTES, Message,
    Profile, Result, SearchSource, VERSION, VERSION_2, valid_origin,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    sync::{Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
};

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);
pub const MAX_CONNECTIONS: usize = 32;
pub const MAX_PENDING: usize = 32;
const OPEN_TTL: Duration = Duration::from_secs(300);
const MAX_OPEN_REFS: usize = 500;
type Pending = HashMap<String, oneshot::Sender<Message>>;
struct State {
    pending: Pending,
    claims: HashMap<String, Instant>,
    references: HashMap<String, Instant>,
    ping: Option<oneshot::Sender<()>>,
}
pub struct Session {
    profile: Profile,
    version: u32,
    state: Mutex<State>,
    output: mpsc::Sender<Message>,
    closed: watch::Sender<bool>,
    ping_gate: tokio::sync::Mutex<()>,
}
impl Session {
    fn new(profile: Profile, version: u32, output: mpsc::Sender<Message>) -> Arc<Self> {
        Arc::new(Self {
            profile,
            version,
            state: Mutex::new(State {
                pending: HashMap::new(),
                claims: HashMap::new(),
                references: HashMap::new(),
                ping: None,
            }),
            output,
            closed: watch::channel(false).0,
            ping_gate: tokio::sync::Mutex::new(()),
        })
    }
    fn conflicted(&self) -> bool {
        let mut state = self.state.lock().expect("session state");
        state.claims.retain(|_, expiry| *expiry > Instant::now());
        !state.claims.is_empty()
    }
    fn claim(&self, id: String) -> Result<()> {
        let mut state = self.state.lock().expect("session state");
        state.claims.retain(|_, expiry| *expiry > Instant::now());
        if state.claims.len() >= 8 {
            return Err(Error(Code::Busy));
        }
        state
            .claims
            .insert(id, Instant::now() + Duration::from_secs(6));
        for (id, sender) in state.pending.drain() {
            let _ = sender.send(self.profile_conflict(&id));
        }
        Ok(())
    }
    fn release(&self, id: &str) {
        self.state.lock().expect("session state").claims.remove(id);
    }
    fn close(&self) {
        self.closed.send_replace(true);
        self.state.lock().expect("session state").pending.clear();
    }
    async fn send(&self, message: Message) -> Result<()> {
        tokio::time::timeout(Duration::from_secs(2), self.output.send(message))
            .await
            .map_err(|_| Error(Code::Timeout))?
            .map_err(|_| Error(Code::Disconnected))
    }
    fn profile_conflict(&self, request_id: &str) -> Message {
        Message {
            v: self.version,
            ..Message::error(request_id, Code::ProfileConflict)
        }
    }
    fn deliver(&self, mut message: Message) {
        // A valid uncorrelated Native error is a notice, never the response to
        // an arbitrary pending request. Its deadline remains unchanged.
        if message.kind == "error" && message.request_id.is_empty() {
            return;
        }
        if self.conflicted() {
            message = self.profile_conflict(&message.request_id);
        }
        let mut state = self.state.lock().expect("session state");
        if message.kind == "pong" {
            if let Some(sender) = state.ping.take() {
                let _ = sender.send(());
            }
            return;
        }
        if let Some(sender) = state.pending.remove(&message.request_id) {
            let _ = sender.send(message);
        }
    }
    async fn probe(&self) -> Result<()> {
        if self.conflicted() {
            return Err(Error(Code::ProfileConflict));
        }
        let _guard = tokio::time::timeout(Duration::from_secs(2), self.ping_gate.lock())
            .await
            .map_err(|_| Error(Code::Busy))?;
        let (sender, receiver) = oneshot::channel();
        self.state.lock().expect("session state").ping = Some(sender);
        self.send(Message {
            v: self.version,
            ..Message::new("ping")
        })
        .await?;
        let result = tokio::time::timeout(Duration::from_secs(2), receiver).await;
        self.state.lock().expect("session state").ping = None;
        if self.conflicted() {
            return Err(Error(Code::ProfileConflict));
        }
        match result {
            Ok(Ok(())) => Ok(()),
            _ => Err(Error(Code::Disconnected)),
        }
    }
    async fn request(self: &Arc<Self>, mut message: Message) -> Result<Message> {
        if self.conflicted() {
            return Err(Error(Code::ProfileConflict));
        }
        let opening = message.kind == "open-page";
        let local_id = message.request_id.clone();
        let native_id = if opening {
            uuid::Uuid::new_v4().to_string()
        } else {
            search_request_id(&local_id)?
        };
        let limit = message.limit;
        let snippets_requested = message.include_snippet == Some(true);
        let (sender, receiver) = oneshot::channel();
        {
            let mut state = self.state.lock().expect("session state");
            state.claims.retain(|_, expiry| *expiry > Instant::now());
            if !state.claims.is_empty() {
                return Err(Error(Code::ProfileConflict));
            }
            if state.pending.len() >= MAX_PENDING {
                return Err(Error(Code::Busy));
            }
            if opening
                && !state
                    .references
                    .remove(&message.open_ref)
                    .is_some_and(|expiry| expiry > Instant::now())
            {
                return Err(Error(Code::ResultExpired));
            }
            state.pending.insert(native_id.clone(), sender);
        }
        let _guard = PendingGuard {
            session: self.clone(),
            id: native_id.clone(),
        };
        message.v = self.version;
        message.request_id = native_id.clone();
        message.profile_id.clear();
        message.native()?;
        self.send(message).await?;
        let mut closed = self.closed.subscribe();
        if *closed.borrow() {
            return Err(Error(Code::Disconnected));
        }
        let mut response = tokio::select! {
            response=tokio::time::timeout(REQUEST_TIMEOUT,receiver)=>response.map_err(|_|Error(Code::Timeout))?.map_err(|_|Error(Code::Disconnected))?,
            _=closed.changed()=>return Err(Error(Code::Disconnected)),
        };
        response.response(&native_id, opening, limit)?;
        if self.conflicted() {
            return Err(Error(Code::ProfileConflict));
        }
        if response.kind == "error" {
            return Err(Error(response.code.unwrap_or(Code::SearchFailed)));
        }
        if !opening {
            if !snippets_requested
                && response
                    .search_result_v2
                    .as_ref()
                    .is_some_and(|result| result.has_snippets())
            {
                return Err(Error(Code::SearchFailed));
            }
            let references = response
                .items
                .iter()
                .map(|item| item.open_ref.as_str())
                .chain(
                    response
                        .search_result_v2
                        .iter()
                        .flat_map(|result| &result.items)
                        .map(|item| item.open_ref.as_str()),
                );
            let mut unique = HashSet::new();
            if references
                .clone()
                .any(|reference| !unique.insert(reference))
            {
                return Err(Error(Code::SearchFailed));
            }
            let mut state = self.state.lock().expect("session state");
            state
                .references
                .retain(|_, expiry| *expiry > Instant::now());
            for reference in references {
                if state.references.len() >= MAX_OPEN_REFS
                    && let Some(old) = state
                        .references
                        .iter()
                        .min_by_key(|(_, expiry)| **expiry)
                        .map(|(key, _)| key.clone())
                {
                    state.references.remove(&old);
                }
                state
                    .references
                    .insert(reference.into(), Instant::now() + OPEN_TTL);
            }
        }
        response.request_id = local_id;
        Ok(response)
    }
    async fn local(self: &Arc<Self>, request: LocalRequest) -> LocalResponse {
        let message = request.message();
        let version = if message.v == VERSION_2 {
            VERSION_2
        } else {
            VERSION
        };
        let request_id = message.request_id.clone();
        let result = async {
            request.validate()?;
            if message.profile_id != self.profile.profile_id {
                return Err(Error(Code::InvalidRequest));
            }
            if version == VERSION_2 && self.version != VERSION_2 {
                return Err(Error(Code::IncompatibleVersion));
            }
            match request {
                LocalRequest::Probe { .. } => {
                    self.probe().await?;
                    Ok(LocalResponse::profile_at_version(
                        version,
                        &request_id,
                        &self.profile,
                    ))
                }
                LocalRequest::Search { .. } | LocalRequest::SearchV2(_) => {
                    let result = self.request(message).await?;
                    local_search_result(version, result)
                }
                LocalRequest::Open { .. } => {
                    self.request(message).await?;
                    Ok(LocalResponse::Open {
                        v: version,
                        request_id: request_id.clone(),
                        ok: true,
                    })
                }
            }
        }
        .await;
        result
            .unwrap_or_else(|error| LocalResponse::error_at_version(version, &request_id, error.0))
    }
}

fn local_search_result(version: u32, message: Message) -> Result<LocalResponse> {
    let (items, has_more) = if let Some(mut result) = message.search_result_v2 {
        if version == VERSION_2 {
            result.request_id = message.request_id;
            return Ok(LocalResponse::SearchV2(result));
        }
        if result.items.is_empty()
            && result
                .partial_sources
                .iter()
                .any(|source| matches!(source, SearchSource::History | SearchSource::Bookmark))
        {
            return Err(Error(Code::SearchFailed));
        }
        let has_more = result.next_cursor.is_some() || !result.partial_sources.is_empty();
        (result.items.into_iter().map(Into::into).collect(), has_more)
    } else {
        (
            message.items,
            message.has_more.ok_or(Error(Code::SearchFailed))?,
        )
    };
    Ok(LocalResponse::Search {
        v: VERSION,
        request_id: message.request_id,
        items,
        has_more,
    })
}

fn search_request_id(local_id: &str) -> Result<String> {
    let id = uuid::Uuid::new_v4();
    let json_bytes = serde_json::to_string(local_id)?.len() - 2;
    if json_bytes <= 36 {
        return Ok(id.to_string());
    }
    // Kwel budgets its page against the Native request ID. Reserve at least
    // as many JSON bytes as the Local ID so restoring it can never cut a page
    // short or invalidate its cursor. Each U+0010–U+001F unit takes six bytes
    // on the wire, covering even a maximally escaped 100-unit Local ID.
    let mut reserved = String::new();
    for byte in id.as_bytes() {
        reserved.push(char::from(16 + (byte >> 4)));
        reserved.push(char::from(16 + (byte & 15)));
    }
    while reserved.len() < json_bytes.div_ceil(6) {
        reserved.push('\u{001f}');
    }
    Ok(reserved)
}

struct PendingGuard {
    session: Arc<Session>,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.session
            .state
            .lock()
            .expect("session state")
            .pending
            .remove(&self.id);
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    token: String,
    operation: String,
    session: String,
}
async fn control(
    paths: &RuntimePaths,
    profile: &str,
    token: &str,
    operation: &str,
    session: &str,
) -> Result<()> {
    let mut stream = ipc::connect(&paths.endpoint(profile, true)?).await?;
    let request = Control {
        token: token.into(),
        operation: operation.into(),
        session: session.into(),
    };

    tokio::time::timeout(Duration::from_secs(1), async {
        ipc::write(&mut *stream, &request).await?;
        let data = ipc::read(&mut *stream, 128).await?;
        if data != b"true" {
            return Err(Error(Code::PermissionRequired));
        }
        Ok(())
    })
    .await
    .map_err(|_| Error(Code::Timeout))?
}
async fn serve_public(mut listener: ipc::Listener, session: Arc<Session>) {
    let semaphore = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut closed = session.closed.subscribe();
    loop {
        if *closed.borrow() {
            break;
        }
        let permit = tokio::select! {result=semaphore.clone().acquire_owned()=>match result{Ok(permit)=>permit,Err(_)=>break},_=closed.changed()=>break};
        let connection =
            tokio::select! {connection=listener.accept()=>connection,_=closed.changed()=>break};
        let Ok(mut stream) = connection else {
            continue;
        };
        let session = session.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let response = match tokio::time::timeout(
                Duration::from_secs(3),
                ipc::read(&mut *stream, MAX_LOCAL_REQUEST_BYTES),
            )
            .await
            {
                Ok(Ok(bytes)) if stream.verify_peer().is_ok() => {
                    match serde_json::from_slice::<LocalRequest>(&bytes) {
                        Ok(request) => session.local(request).await,
                        Err(_) => LocalResponse::error("", Code::InvalidRequest),
                    }
                }
                _ => LocalResponse::error("", Code::InvalidRequest),
            };
            let _ =
                tokio::time::timeout(Duration::from_secs(2), ipc::write(&mut *stream, &response))
                    .await;
        });
    }
}
async fn serve_control(mut listener: ipc::Listener, session: Arc<Session>, token: String) {
    let semaphore = Arc::new(Semaphore::new(4));
    let mut closed = session.closed.subscribe();
    loop {
        if *closed.borrow() {
            break;
        }
        let permit = tokio::select! {result=semaphore.clone().acquire_owned()=>match result{Ok(permit)=>permit,Err(_)=>break},_=closed.changed()=>break};
        let connection =
            tokio::select! {connection=listener.accept()=>connection,_=closed.changed()=>break};
        let Ok(mut stream) = connection else {
            continue;
        };
        let session = session.clone();
        let token = token.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let operation =
                tokio::time::timeout(Duration::from_secs(1), ipc::read(&mut *stream, 1024)).await;
            let ok = match operation {
                Ok(Ok(bytes)) if stream.verify_peer().is_ok() => {
                    match serde_json::from_slice::<Control>(&bytes) {
                        Ok(request)
                            if request.token.as_bytes().ct_eq(token.as_bytes()).into()
                                && kwel_protocol::id(&request.session) =>
                        {
                            match request.operation.as_str() {
                                "claim" => session.claim(request.session).is_ok(),
                                "release" => {
                                    session.release(&request.session);
                                    true
                                }
                                _ => false,
                            }
                        }
                        _ => false,
                    }
                }
                _ => false,
            };
            let _ =
                tokio::time::timeout(Duration::from_secs(1), ipc::write(&mut *stream, &ok)).await;
        });
    }
}
/// All stdout access is owned by the binary's framed writer. No diagnostic
/// payload, query, URL, label or profile ID is sent to the logging interface.
pub async fn run_transport(
    origin: &str,
    paths: RuntimePaths,
    mut input: mpsc::Receiver<Result<Message>>,
    output: mpsc::Sender<Message>,
) -> Result<()> {
    if !valid_origin(origin, EXTENSION_ID) {
        return Err(Error(Code::PermissionRequired));
    }
    let hello = tokio::time::timeout(Duration::from_secs(5), input.recv())
        .await
        .map_err(|_| Error(Code::Timeout))?
        .ok_or(Error(Code::Disconnected))??;
    hello.native()?;
    hello.hello()?;
    let profile = hello.profile_id.to_lowercase();
    let lock = secure::lock(&paths.file(&profile, "lock")?)?;
    if lock.try_lock().is_err() {
        let token = String::from_utf8(secure::read(&paths.file(&profile, "token")?)?)
            .map_err(|_| Error(Code::UnsafePath))?;
        let claim = uuid::Uuid::new_v4().to_string();
        control(&paths, &profile, &token, "claim", &claim).await?;
        // The duplicate never receives hello-ack. Its lease outlives the five
        // second Chrome handshake, including an unexpected process exit.
        let _ = tokio::time::timeout(Duration::from_secs(5), input.recv()).await;
        let _ = control(&paths, &profile, &token, "release", &claim).await;
        return Err(Error(Code::ProfileConflict));
    }
    let public = ipc::Listener::bind(&paths.endpoint(&profile, false)?)?;
    let private = ipc::Listener::bind(&paths.endpoint(&profile, true)?)?;
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    secure::atomic_write(&paths.file(&profile, "token")?, token.as_bytes(), false)?;
    #[cfg(windows)]
    secure::atomic_write(&paths.file(&profile, "endpoint")?, b"1", false)?;
    let session = Session::new(
        Profile {
            profile_id: profile.clone(),
            profile_label: hello.profile_label,
            host_version: crate::VERSION.into(),
        },
        hello.v,
        output,
    );
    let tasks: [JoinHandle<()>; 2] = [
        tokio::spawn(serve_public(public, session.clone())),
        tokio::spawn(serve_control(private, session.clone(), token)),
    ];
    let result = async {
        session
            .send(Message {
                v: session.version,
                ..Message::new("hello-ack")
            })
            .await?;
        loop {
            let message = tokio::select! {
                message = input.recv() => match message {Some(message) => message, None => break},
                _ = session.output.closed() => return Err(Error(Code::Disconnected)),
            };
            let message = message?;
            message.native()?;
            if message.v != session.version {
                return Err(Error(Code::IncompatibleVersion));
            }
            if !matches!(
                message.kind.as_str(),
                "pong" | "search-result" | "open-result" | "error"
            ) {
                return Err(Error(Code::InvalidRequest));
            }
            session.deliver(message);
        }
        Ok(())
    }
    .await;
    session.close();
    for task in tasks {
        let _ = task.await;
    }
    // Ownership lock remains held until both listeners have removed their own
    // endpoints. Neither a client nor a previous process can delete replacements.
    secure::remove_regular(&paths.file(&profile, "token")?)?;
    #[cfg(windows)]
    secure::remove_regular(&paths.file(&profile, "endpoint")?)?;
    drop(lock);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn session() -> (Arc<Session>, mpsc::Receiver<Message>) {
        let (output, receiver) = mpsc::channel(40);
        (
            Session::new(
                Profile {
                    profile_id: "00000000-0000-4000-8000-00000000000a".into(),
                    profile_label: "Fixture".into(),
                    host_version: crate::VERSION.into(),
                },
                VERSION,
                output,
            ),
            receiver,
        )
    }
    fn search() -> Message {
        Message {
            request_id: "local".into(),
            query: "Kyoto".into(),
            limit: 20,
            ..Message::new("search")
        }
    }
    #[tokio::test]
    async fn pending_requests_are_bounded_and_disconnect_cancels_all() {
        let (session, mut output) = session();
        let mut jobs = Vec::new();
        for _ in 0..MAX_PENDING {
            let copy = session.clone();
            jobs.push(tokio::spawn(async move { copy.request(search()).await }));
        }
        for _ in 0..MAX_PENDING {
            output.recv().await.unwrap();
        }
        assert!(matches!(
            session.request(search()).await,
            Err(Error(Code::Busy))
        ));
        session.close();
        for job in jobs {
            assert!(matches!(job.await.unwrap(), Err(Error(Code::Disconnected))));
        }
        assert!(session.state.lock().unwrap().pending.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn uncorrelated_errors_do_not_consume_pending_requests_or_extend_deadlines() {
        let (session, mut output) = session();
        let copy = session.clone();
        let job = tokio::spawn(async move { copy.request(search()).await });
        let request = output.recv().await.unwrap();
        tokio::time::advance(REQUEST_TIMEOUT / 2).await;
        let notice = kwel_protocol::decode(
            br#"{"v":1,"type":"error","requestId":null,"code":"BAD_REQUEST","message":"Notice"}"#,
        )
        .unwrap();
        session.deliver(notice);
        {
            let state = session.state.lock().unwrap();
            assert_eq!(state.pending.len(), 1);
            assert!(state.pending.contains_key(&request.request_id));
        }
        assert!(!job.is_finished());
        tokio::time::advance(REQUEST_TIMEOUT / 2 + Duration::from_millis(1)).await;
        assert!(matches!(job.await.unwrap(), Err(Error(Code::Timeout))));
        assert!(session.state.lock().unwrap().pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn request_timeout_unregisters_correlation_and_rejects_late_response() {
        let (session, mut output) = session();
        let copy = session.clone();
        let job = tokio::spawn(async move { copy.request(search()).await });
        let request = output.recv().await.unwrap();
        tokio::time::advance(REQUEST_TIMEOUT + Duration::from_millis(1)).await;
        assert!(matches!(job.await.unwrap(), Err(Error(Code::Timeout))));
        assert!(session.state.lock().unwrap().pending.is_empty());
        session.deliver(Message {
            request_id: request.request_id,
            has_more: Some(false),
            ..Message::new("search-result")
        });
        assert!(session.state.lock().unwrap().references.is_empty());
    }
    #[tokio::test]
    async fn canceled_request_cannot_deliver_its_late_response_to_the_next_search() {
        let (session, mut output) = session();
        let copy = session.clone();
        let old = tokio::spawn(async move { copy.request(search()).await });
        let first = output.recv().await.unwrap();
        old.abort();
        assert!(old.await.unwrap_err().is_cancelled());
        assert!(session.state.lock().unwrap().pending.is_empty());
        let copy = session.clone();
        let next = tokio::spawn(async move { copy.request(search()).await });
        let second = output.recv().await.unwrap();
        assert_ne!(first.request_id, second.request_id);
        session.deliver(Message {
            request_id: first.request_id,
            has_more: Some(false),
            ..Message::new("search-result")
        });
        assert!(!next.is_finished());
        session.deliver(Message {
            request_id: second.request_id,
            has_more: Some(false),
            ..Message::new("search-result")
        });
        assert_eq!(next.await.unwrap().unwrap().request_id, "local");
    }
    #[tokio::test]
    async fn expired_and_cross_session_open_refs_never_reach_chrome() {
        let (session, mut output) = session();
        let reference = "00000000-0000-4000-8000-000000000101";
        session
            .state
            .lock()
            .unwrap()
            .references
            .insert(reference.into(), Instant::now() - Duration::from_secs(1));
        let open = Message {
            request_id: "open".into(),
            profile_id: session.profile.profile_id.clone(),
            open_ref: reference.into(),
            ..Message::new("open-page")
        };
        assert!(matches!(
            session.request(open.clone()).await,
            Err(Error(Code::ResultExpired))
        ));
        let (other, _) = self::session();
        assert!(matches!(
            other.request(open).await,
            Err(Error(Code::ResultExpired))
        ));
        assert!(output.try_recv().is_err());
    }
}
