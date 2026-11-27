mod support;
use kwel_native_host::{ipc, paths::RuntimePaths, server};
use kwel_protocol::{
    Code, EXTENSION_ID, Error, Item, LocalResponse, MAX_MESSAGE_BYTES, Message, Result,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::sync::mpsc;
const A: &str = "00000000-0000-4000-8000-00000000000a";
const B: &str = "00000000-0000-4000-8000-00000000000b";
const REF: &str = "00000000-0000-4000-8000-000000000101";
struct Host {
    _directory: tempfile::TempDir,
    paths: RuntimePaths,
    input: mpsc::Sender<Result<Message>>,
    output: mpsc::Receiver<Message>,
    task: tokio::task::JoinHandle<Result<()>>,
}
impl Host {
    async fn start(profile: &str) -> Self {
        Self::start_at_version(profile, 1).await
    }

    async fn start_at_version(profile: &str, v: u32) -> Self {
        let directory = support::directory();
        let paths =
            RuntimePaths::at(directory.path().canonicalize().unwrap().join("runtime")).unwrap();
        let (input, receiver) = mpsc::channel(4);
        let (sender, mut output) = mpsc::channel(32);
        let copy = paths.clone();
        let origin = format!("chrome-extension://{EXTENSION_ID}/");
        let task =
            tokio::spawn(
                async move { server::run_transport(&origin, copy, receiver, sender).await },
            );
        input
            .send(Ok(Message {
                v,
                profile_id: profile.into(),
                profile_label: "Fixture".into(),
                ..Message::new("hello")
            }))
            .await
            .unwrap();
        let ack = tokio::time::timeout(Duration::from_secs(3), output.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((ack.v, ack.kind.as_str()), (v, "hello-ack"));
        Self {
            _directory: directory,
            paths,
            input,
            output,
            task,
        }
    }
    async fn next(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(3), self.output.recv())
            .await
            .unwrap()
            .unwrap()
    }
    async fn stop(self) {
        drop(self.input);
        assert!(
            tokio::time::timeout(Duration::from_secs(3), self.task)
                .await
                .unwrap()
                .unwrap()
                .is_ok()
        );
    }
}
async fn exchange(paths: RuntimePaths, profile: &str, request: serde_json::Value) -> LocalResponse {
    let mut connection = ipc::connect(&paths.endpoint(profile, false).unwrap())
        .await
        .unwrap();
    ipc::write(&mut *connection, &request).await.unwrap();
    let payload = ipc::read(&mut *connection, MAX_MESSAGE_BYTES)
        .await
        .unwrap();
    serde_json::from_slice(&payload).unwrap()
}
fn search(profile: &str, request: &str) -> serde_json::Value {
    serde_json::json!({"v":1,"type":"search","profileId":profile,"requestId":request,"query":"Kyoto 日本語 ! $()","limit":20})
}
fn result(id: &str) -> Message {
    Message {
        request_id: id.into(),
        has_more: Some(true),
        items: vec![Item {
            title: "Second-ranked".into(),
            url: "https://example.com/2".into(),
            domain: "example.com".into(),
            open_ref: REF.into(),
        }],
        ..Message::new("search-result")
    }
}

fn rich_result(request_id: &str, snippet: bool) -> Value {
    let mut item = json!({
        "title":"Reading notes","url":"https://example.com/notes","domain":"example.com",
        "openRef":REF,"matchedIn":["body"]
    });
    if snippet {
        item["snippet"] = json!("Kyoto 日本語");
    }
    json!({
        "v":2,"type":"search-result","requestId":request_id,"items":[item],
        "nextCursor":"continuation","partialSources":["body"]
    })
}

#[tokio::test]
async fn native_v2_relays_opted_in_metadata_and_projects_v1_on_the_same_port() {
    let mut host = Host::start_at_version(A, 2).await;
    let mut rich = search(A, "rich");
    rich["v"] = json!(2);
    rich["cursor"] = json!("previous");
    rich["includeSnippet"] = json!(true);
    let task = tokio::spawn(exchange(host.paths.clone(), A, rich));
    let request = host.next().await;
    assert_eq!(request.v, 2);
    assert_eq!(request.cursor.as_deref(), Some("previous"));
    assert_eq!(request.include_snippet, Some(true));
    assert_ne!(request.request_id, "rich");
    assert!(request.profile_id.is_empty());
    let mut reply = rich_result(&request.request_id, true);
    host.input
        .send(kwel_protocol::decode_native(
            &serde_json::to_vec(&reply).unwrap(),
        ))
        .await
        .unwrap();
    reply["requestId"] = json!("rich");
    assert_eq!(serde_json::to_value(task.await.unwrap()).unwrap(), reply);

    let task = tokio::spawn(exchange(host.paths.clone(), A, search(A, "legacy")));
    let request = host.next().await;
    assert_eq!(request.v, 2);
    assert_eq!((request.cursor, request.include_snippet), (None, None));
    host.input
        .send(kwel_protocol::decode_native(
            &serde_json::to_vec(&rich_result(&request.request_id, false)).unwrap(),
        ))
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(task.await.unwrap()).unwrap(),
        json!({"v":1,"type":"search-result","requestId":"legacy","hasMore":true,
            "items":[{"title":"Reading notes","url":"https://example.com/notes","domain":"example.com","openRef":REF}]})
    );

    let open = json!({"v":2,"type":"open-page","profileId":A,"requestId":"open","openRef":REF});
    let task = tokio::spawn(exchange(host.paths.clone(), A, open.clone()));
    let request = host.next().await;
    assert_eq!((request.v, request.open_ref.as_str()), (2, REF));
    host.input
        .send(Ok(Message {
            v: 2,
            request_id: request.request_id,
            ok: true,
            ..Message::new("open-result")
        }))
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        LocalResponse::Open { v: 2, ok: true, .. }
    ));
    assert!(matches!(
        exchange(host.paths.clone(), A, open).await,
        LocalResponse::Error {
            v: 2,
            code: Code::ResultExpired,
            ..
        }
    ));
    host.stop().await;
}

#[tokio::test]
async fn v2_requests_on_a_v1_port_report_incompatibility_without_forwarding() {
    let mut host = Host::start(A).await;
    let mut rich = search(A, "rich");
    rich["v"] = json!(2);
    rich["includeSnippet"] = json!(true);
    let probe = json!({"v":2,"type":"profile-probe","requestId":"probe","profileId":A});
    for (request, id) in [(rich, "rich"), (probe, "probe")] {
        assert!(matches!(exchange(host.paths.clone(), A, request).await,
            LocalResponse::Error { v: 2, code: Code::IncompatibleVersion, request_id, .. } if request_id == id));
    }
    assert!(host.output.try_recv().is_err());
    host.stop().await;
}

#[tokio::test]
async fn wrong_native_version_closes_the_session_without_completing_search() {
    let mut host = Host::start_at_version(A, 2).await;
    let task = tokio::spawn(exchange(host.paths.clone(), A, search(A, "legacy")));
    let request = host.next().await;
    host.input
        .send(Ok(result(&request.request_id)))
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        LocalResponse::Error {
            v: 1,
            code: Code::Disconnected,
            ..
        }
    ));
    assert_eq!(
        host.task.await.unwrap(),
        Err(Error(Code::IncompatibleVersion))
    );
}

#[tokio::test]
async fn unsolicited_snippets_cannot_reach_local_clients_or_issue_references() {
    let mut host = Host::start_at_version(A, 2).await;
    let mut search = search(A, "no-snippet");
    search["v"] = json!(2);
    let task = tokio::spawn(exchange(host.paths.clone(), A, search));
    let request = host.next().await;
    host.input
        .send(kwel_protocol::decode_native(
            &serde_json::to_vec(&rich_result(&request.request_id, true)).unwrap(),
        ))
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        LocalResponse::Error {
            v: 2,
            code: Code::SearchFailed,
            ..
        }
    ));
    let open = json!({"v":2,"type":"open-page","requestId":"open","profileId":A,"openRef":REF});
    assert!(matches!(
        exchange(host.paths.clone(), A, open).await,
        LocalResponse::Error {
            v: 2,
            code: Code::ResultExpired,
            ..
        }
    ));
    assert!(host.output.try_recv().is_err());
    host.stop().await;
}

#[tokio::test]
async fn restoring_a_maximally_escaped_local_id_preserves_a_full_frame_and_cursor() {
    let mut host = Host::start_at_version(A, 2).await;
    let local_id = "\0".repeat(kwel_protocol::MAX_REQUEST_ID_UNITS);
    let mut request = search(A, &local_id);
    request["v"] = json!(2);
    let task = tokio::spawn(exchange(host.paths.clone(), A, request));
    let request = host.next().await;
    assert_ne!(request.request_id, local_id);
    let mut reply = rich_result(&request.request_id, false);
    let padding = MAX_MESSAGE_BYTES - serde_json::to_vec(&reply).unwrap().len();
    reply["items"][0]["title"] = json!(format!("Reading notes{}", "x".repeat(padding)));
    let bytes = serde_json::to_vec(&reply).unwrap();
    assert_eq!(bytes.len(), MAX_MESSAGE_BYTES);
    host.input
        .send(kwel_protocol::decode_native(&bytes))
        .await
        .unwrap();
    let response = serde_json::to_value(task.await.unwrap()).unwrap();
    assert_eq!(response["requestId"], local_id);
    assert_eq!(response["nextCursor"], reply["nextCursor"]);
    assert_eq!(response["items"], reply["items"]);
    assert_eq!(
        serde_json::to_vec(&response).unwrap().len(),
        MAX_MESSAGE_BYTES
    );
    host.stop().await;
}

#[tokio::test]
async fn search_open_correlation_single_use_and_profile_isolation() {
    let mut host = Host::start(A).await;
    let paths = host.paths.clone();
    let task = tokio::spawn(exchange(paths, A, search(A, "same-client-id")));
    let request = host.next().await;
    assert_eq!(request.kind, "search");
    assert_eq!(request.query, "Kyoto 日本語 ! $()");
    assert_eq!(request.limit, 20);
    assert!(request.profile_id.is_empty());
    assert_ne!(request.request_id, "same-client-id");
    host.input
        .send(Ok(result(&request.request_id)))
        .await
        .unwrap();
    assert!(
        matches!(task.await.unwrap(),LocalResponse::Search{request_id,has_more:true,items,..} if request_id=="same-client-id"&&items[0].open_ref==REF)
    );
    let wrong = exchange(host.paths.clone(), A, search(B, "wrong")).await;
    assert!(matches!(
        wrong,
        LocalResponse::Error {
            code: Code::InvalidRequest,
            ..
        }
    ));
    let open = serde_json::json!({"v":1,"type":"open-page","profileId":A,"requestId":"open","openRef":REF});
    let task = tokio::spawn(exchange(host.paths.clone(), A, open.clone()));
    let request = host.next().await;
    assert_eq!(request.kind, "open-page");
    assert_eq!(request.open_ref, REF);
    assert!(request.profile_id.is_empty());
    host.input
        .send(Ok(Message {
            request_id: request.request_id,
            ok: true,
            ..Message::new("open-result")
        }))
        .await
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        LocalResponse::Open { ok: true, .. }
    ));
    assert!(matches!(
        exchange(host.paths.clone(), A, open).await,
        LocalResponse::Error {
            code: Code::ResultExpired,
            ..
        }
    ));
    host.stop().await;
}
#[tokio::test]
async fn concurrent_clients_can_reuse_request_ids_without_mixing_responses() {
    let mut host = Host::start(A).await;
    let mut first_query = search(A, "r");
    first_query["query"] = "one".into();
    let mut second_query = search(A, "r");
    second_query["query"] = "two".into();
    let one = tokio::spawn(exchange(host.paths.clone(), A, first_query));
    let two = tokio::spawn(exchange(host.paths.clone(), A, second_query));
    let first = host.next().await;
    let second = host.next().await;
    assert_ne!(first.request_id, second.request_id);
    for request in [second, first] {
        let mut response = result(&request.request_id);
        response.items[0].title = request.query;
        host.input.send(Ok(response)).await.unwrap();
    }
    for (task, expected) in [(one, "one"), (two, "two")] {
        assert!(
            matches!(task.await.unwrap(),LocalResponse::Search{request_id,items,..} if request_id=="r" && items[0].title==expected)
        );
    }
    host.stop().await;
}

#[tokio::test]
async fn uncorrelated_errors_never_complete_another_request_or_disconnect_the_session() {
    let mut host = Host::start(A).await;
    let one = tokio::spawn(exchange(host.paths.clone(), A, search(A, "one")));
    let first = host.next().await;
    let two = tokio::spawn(exchange(host.paths.clone(), A, search(A, "two")));
    let second = host.next().await;
    for code in [
        "BAD_REQUEST",
        "INCOMPATIBLE_VERSION",
        "NOT_READY",
        "SEARCH_FAILED",
        "RESULT_EXPIRED",
        "OPEN_FAILED",
    ] {
        let bytes = serde_json::to_vec(
            &serde_json::json!({"v":1,"type":"error","requestId":null,"code":code}),
        )
        .unwrap();
        host.input
            .send(kwel_protocol::decode(&bytes))
            .await
            .unwrap();
    }
    host.input
        .send(Ok(Message::error(&second.request_id, Code::BadRequest)))
        .await
        .unwrap();
    host.input
        .send(Ok(result(&first.request_id)))
        .await
        .unwrap();
    assert!(
        matches!(one.await.unwrap(),LocalResponse::Search {request_id,..} if request_id=="one")
    );
    assert!(
        matches!(two.await.unwrap(),LocalResponse::Error {request_id,code:Code::BadRequest,..} if request_id=="two")
    );
    let next = tokio::spawn(exchange(host.paths.clone(), A, search(A, "after-error")));
    let request = host.next().await;
    host.input
        .send(Ok(result(&request.request_id)))
        .await
        .unwrap();
    assert!(
        matches!(next.await.unwrap(),LocalResponse::Search {request_id,..} if request_id=="after-error")
    );
    host.stop().await;
}

#[tokio::test]
async fn malformed_success_response_fails_closed_and_cancels_pending_requests() {
    let mut host = Host::start(A).await;
    let paths = host.paths.clone();
    let request = tokio::spawn(exchange(paths.clone(), A, search(A, "pending")));
    host.next().await;
    host.input
        .send(Ok(Message {
            has_more: Some(false),
            ..Message::new("search-result")
        }))
        .await
        .unwrap();
    assert!(matches!(
        request.await.unwrap(),
        LocalResponse::Error {
            code: Code::Disconnected,
            ..
        }
    ));
    drop(host.input);
    assert!(matches!(
        host.task.await.unwrap(),
        Err(Error(Code::InvalidRequest))
    ));
    assert!(
        ipc::connect(&paths.endpoint(A, false).unwrap())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn null_and_unknown_native_ids_cannot_take_another_clients_response() {
    let mut host = Host::start(A).await;
    let mut one_request = search(A, "one-local");
    one_request["query"] = "one".into();
    let mut two_request = search(A, "two-local");
    two_request["query"] = "two".into();
    let one = tokio::spawn(exchange(host.paths.clone(), A, one_request));
    let two = tokio::spawn(exchange(host.paths.clone(), A, two_request));
    let first = host.next().await;
    let second = host.next().await;
    assert_ne!(first.request_id, second.request_id);
    assert!(first.profile_id.is_empty() && second.profile_id.is_empty());
    let notice = kwel_protocol::decode(
        br#"{"v":1,"type":"error","requestId":null,"code":"NOT_READY","message":"Notice"}"#,
    )
    .unwrap();
    host.input.send(Ok(notice)).await.unwrap();
    host.input
        .send(Ok(result("unknown-native-id")))
        .await
        .unwrap();
    for request in [second, first] {
        let mut response = result(&request.request_id);
        response.items[0].title = request.query;
        host.input.send(Ok(response)).await.unwrap();
    }
    for (task, local_id, title) in [(one, "one-local", "one"), (two, "two-local", "two")] {
        assert!(
            matches!(task.await.unwrap(), LocalResponse::Search { request_id, items, .. } if request_id==local_id && items[0].title==title)
        );
    }
    host.stop().await;
}

#[tokio::test]
async fn failed_native_writer_releases_both_endpoints_without_another_input() {
    let host = Host::start(A).await;
    let paths = host.paths.clone();
    drop(host.output);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(3), host.task)
            .await
            .unwrap()
            .unwrap(),
        Err(Error(Code::Disconnected))
    ));
    assert!(
        ipc::connect(&paths.endpoint(A, false).unwrap())
            .await
            .is_err()
    );
    assert!(
        ipc::connect(&paths.endpoint(A, true).unwrap())
            .await
            .is_err()
    );
}
#[tokio::test]
async fn invalid_hello_never_publishes_an_endpoint() {
    for hello in [
        Message::new("ping"),
        Message {
            profile_id: "invalid".into(),
            ..Message::new("hello")
        },
        Message {
            v: 3,
            profile_id: A.into(),
            ..Message::new("hello")
        },
    ] {
        let directory = support::directory();
        let paths =
            RuntimePaths::at(directory.path().canonicalize().unwrap().join("runtime")).unwrap();
        let (input, receiver) = mpsc::channel(4);
        let (sender, mut output) = mpsc::channel(4);
        input.send(Ok(hello)).await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            server::run_transport(
                &format!("chrome-extension://{EXTENSION_ID}/"),
                paths.clone(),
                receiver,
                sender,
            ),
        )
        .await
        .expect("invalid hello must stop the transport");
        assert!(result.is_err());
        assert!(output.recv().await.is_none());
        assert!(paths.profiles().unwrap().is_empty());
    }
}
#[tokio::test]
async fn oversized_local_header_is_rejected_before_native_forwarding() {
    use tokio::io::AsyncWriteExt;
    let mut host = Host::start(A).await;
    let mut connection = ipc::connect(&host.paths.endpoint(A, false).unwrap())
        .await
        .unwrap();
    connection
        .write_all(&((kwel_protocol::MAX_LOCAL_REQUEST_BYTES + 1) as u32).to_le_bytes())
        .await
        .unwrap();
    let bytes = ipc::read(&mut *connection, MAX_MESSAGE_BYTES)
        .await
        .unwrap();
    assert!(matches!(
        serde_json::from_slice::<LocalResponse>(&bytes).unwrap(),
        LocalResponse::Error {
            code: Code::InvalidRequest,
            ..
        }
    ));
    assert!(host.output.try_recv().is_err());
    host.stop().await;
}
#[tokio::test]
async fn disconnect_fails_pending_requests_and_removes_owned_endpoints() {
    let mut host = Host::start(A).await;
    let paths = host.paths.clone();
    let task = tokio::spawn(exchange(paths.clone(), A, search(A, "r")));
    host.next().await;
    host.stop().await;
    assert!(matches!(
        task.await.unwrap(),
        LocalResponse::Error {
            code: Code::Disconnected,
            ..
        }
    ));
    #[cfg(unix)]
    assert!(!paths.endpoint(A, false).unwrap().exists());
}
#[tokio::test]
async fn duplicate_profile_fails_closed_and_recovers_after_duplicate_disconnect() {
    let mut host = Host::start(A).await;
    let (input, receiver) = mpsc::channel(4);
    let (sender, mut output) = mpsc::channel(4);
    let paths = host.paths.clone();
    let second = tokio::spawn(async move {
        server::run_transport(
            &format!("chrome-extension://{EXTENSION_ID}/"),
            paths,
            receiver,
            sender,
        )
        .await
    });
    input
        .send(Ok(Message {
            profile_id: A.into(),
            ..Message::new("hello")
        }))
        .await
        .unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        exchange(host.paths.clone(), A, search(A, "conflict")),
    )
    .await
    .unwrap();
    assert!(matches!(
        response,
        LocalResponse::Error {
            code: Code::ProfileConflict,
            ..
        }
    ));
    assert!(output.try_recv().is_err());
    drop(input);
    assert!(matches!(
        second.await.unwrap(),
        Err(Error(Code::ProfileConflict))
    ));
    // A request that reached Chrome before the duplicate claim is deliberately
    // unanswered above. Discard that old frame, without a timing-based sleep.
    while host.output.try_recv().is_ok() {}
    let task = tokio::spawn(exchange(host.paths.clone(), A, search(A, "recovered")));
    let request = host.next().await;
    host.input
        .send(Ok(result(&request.request_id)))
        .await
        .unwrap();
    assert!(matches!(task.await.unwrap(), LocalResponse::Search { .. }));
    host.stop().await;
}
#[tokio::test]
async fn public_port_rejects_management_and_private_port_requires_capability() {
    let host = Host::start(A).await;
    for operation in ["uninstall", "profile-claim", "profile-release", "doctor"] {
        assert!(matches!(
            exchange(
                host.paths.clone(),
                A,
                serde_json::json!({"v":1,"type":operation,"requestId":"r","profileId":A})
            )
            .await,
            LocalResponse::Error {
                code: Code::InvalidRequest,
                ..
            }
        ));
    }
    let mut stream = ipc::connect(&host.paths.endpoint(A, true).unwrap())
        .await
        .unwrap();
    ipc::write(
        &mut *stream,
        &serde_json::json!({"token":"incorrect","operation":"claim","session":B}),
    )
    .await
    .unwrap();
    assert_eq!(ipc::read(&mut *stream, 128).await.unwrap(), b"false");
    host.stop().await;
}
#[tokio::test]
async fn unsafe_origin_never_starts_a_host() {
    let directory = support::directory();
    let paths = RuntimePaths::at(directory.path().canonicalize().unwrap().join("runtime")).unwrap();
    let (_input, receiver) = mpsc::channel(1);
    let (sender, _output) = mpsc::channel(1);
    assert!(matches!(
        server::run_transport(
            "chrome-extension://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
            paths,
            receiver,
            sender
        )
        .await,
        Err(Error(Code::PermissionRequired))
    ));
}
#[test]
fn stdio_uses_binary_framing_including_windows_control_z_header() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let profile = uuid::Uuid::new_v4().to_string();
    let payload = serde_json::to_vec(&Message {
        profile_id: profile.clone(),
        profile_label: "Fixture".into(),
        ..Message::new("hello")
    })
    .unwrap();
    let mut padded = payload;
    while padded.len() % 256 != 0x1a {
        padded.push(b' ');
    }
    padded.extend_from_slice(b"\r\n");
    while padded.len() % 256 != 0x1a {
        padded.push(b' ');
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_kwel-native-host"))
        .arg(format!("chrome-extension://{EXTENSION_ID}/"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin
        .write_all(&(padded.len() as u32).to_le_bytes())
        .unwrap();
    stdin.write_all(&padded).unwrap();
    stdin.flush().unwrap();
    let bytes =
        kwel_protocol::read_frame(&mut child.stdout.take().unwrap(), MAX_MESSAGE_BYTES).unwrap();
    assert_eq!(kwel_protocol::decode(&bytes).unwrap().kind, "hello-ack");
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(!diagnostics.contains(&profile));
    assert!(!diagnostics.contains("Fixture"));
}
