use kwel_protocol::{Code, Error, LocalRequest, LocalResponse, decode, decode_native};
use serde_json::{Value, json, to_vec};

const PROFILE: &str = "00000000-0000-4000-8000-000000000001";
const REF: &str = "00000000-0000-4000-8000-000000000002";

fn validators() -> (jsonschema::Validator, jsonschema::Validator) {
    let native: Value = serde_json::from_str(include_str!(
        "../../../docs/schemas/native-messaging-v2.schema.json"
    ))
    .unwrap();
    let local: Value = serde_json::from_str(include_str!(
        "../../../docs/schemas/local-client-v2.schema.json"
    ))
    .unwrap();
    jsonschema::draft202012::meta::validate(&native).unwrap();
    jsonschema::draft202012::meta::validate(&local).unwrap();
    let registry = jsonschema::Registry::new()
        .add(
            native["$id"].as_str().unwrap(),
            jsonschema::Resource::from_contents(native.clone()),
        )
        .unwrap()
        .prepare()
        .unwrap();
    let local = jsonschema::draft202012::options()
        .with_registry(&registry)
        .build(&local)
        .unwrap();
    (jsonschema::draft202012::new(&native).unwrap(), local)
}

fn search() -> Value {
    json!({"v":2,"type":"search","requestId":"r","query":"Kyoto","limit":20,
        "cursor":"opaque","includeSnippet":true})
}

fn result() -> Value {
    json!({"v":2,"type":"search-result","requestId":"r","nextCursor":null,"partialSources":["body"],
        "items":[{"title":"Travel notes","url":"https://example.com/","domain":"",
            "openRef":REF,"matchedIn":["title","body"],"snippet":"Kyoto"}]})
}

#[test]
fn schemas_and_wire_parsers_agree_on_v2_examples_and_closed_shapes() {
    let (native, local) = validators();
    for (document, validator) in [
        (
            include_str!("../../../docs/native-messaging-v2.md"),
            &native,
        ),
        (
            include_str!("../../../docs/local-client-protocol-v2.md"),
            &local,
        ),
    ] {
        let document = document.replace("\r\n", "\n");
        for block in document.split("```json\n").skip(1) {
            let value: Value = serde_json::from_str(block.split_once("```").unwrap().0).unwrap();
            validator.validate(&value).unwrap();
        }
    }

    for value in [
        json!({"v":2,"type":"hello","profileId":PROFILE}),
        json!({"v":2,"type":"hello-ack"}),
        json!({"v":2,"type":"ping"}),
        json!({"v":2,"type":"pong"}),
        search(),
        result(),
        json!({"v":2,"type":"open-page","requestId":"r","openRef":REF}),
        json!({"v":2,"type":"open-result","requestId":"r","ok":true}),
        json!({"v":2,"type":"error","requestId":null,"code":"BAD_REQUEST","message":"Invalid request."}),
    ] {
        native.validate(&value).unwrap();
        let wire = to_vec(&value).unwrap();
        assert_eq!(
            serde_json::to_value(decode_native(&wire).unwrap()).unwrap(),
            value
        );
        assert_eq!(decode(&wire), Err(Error(Code::IncompatibleVersion)));
        let mut unknown = value;
        unknown["extra"] = json!(true);
        assert!(!native.is_valid(&unknown));
        assert!(decode_native(&to_vec(&unknown).unwrap()).is_err());
    }

    for (field, invalid) in [("cursor", Value::Null), ("includeSnippet", json!("true"))] {
        let mut wire = search();
        wire[field] = invalid;
        assert!(!native.is_valid(&wire));
        assert!(decode_native(&to_vec(&wire).unwrap()).is_err());
        wire["profileId"] = json!(PROFILE);
        assert!(!local.is_valid(&wire));
        assert!(serde_json::from_value::<LocalRequest>(wire).is_err());
    }

    let mut local_search = search();
    local_search["profileId"] = json!(PROFILE);
    local.validate(&local_search).unwrap();
    let request: LocalRequest = serde_json::from_value(local_search.clone()).unwrap();
    request.validate().unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), local_search);
    local_search["v"] = json!(1);
    assert!(serde_json::from_value::<LocalRequest>(local_search).is_err());

    let mut wrong_version = search();
    wrong_version["v"] = json!(1);
    assert!(decode_native(&to_vec(&wrong_version).unwrap()).is_err());
    assert!(
        decode_native(
            br#"{"v":2,"type":"search","requestId":"r","query":"","cursor":"a","cursor":"b"}"#
        )
        .is_err()
    );
    assert!(serde_json::from_str::<LocalResponse>(r#"{"v":2,"type":"search-result","requestId":"r","items":[],"nextCursor":null,"nextCursor":null,"partialSources":[]}"#).is_err());
}

#[test]
fn v2_search_results_require_complete_metadata_and_allow_empty_partial_pages() {
    let (native, local) = validators();
    for (field, invalid) in [
        ("nextCursor", json!("")),
        ("partialSources", json!(["url"])),
        ("partialSources", json!(["body", "body"])),
        (
            "items",
            json!([{"title":"t","url":"https://example.com/","domain":"","openRef":REF}]),
        ),
    ] {
        let mut value = result();
        value[field] = invalid;
        assert!(!native.is_valid(&value) && !local.is_valid(&value));
        assert!(decode_native(&to_vec(&value).unwrap()).is_err());
    }
    for missing in ["nextCursor", "partialSources"] {
        let mut value = result();
        value.as_object_mut().unwrap().remove(missing);
        assert!(!native.is_valid(&value) && !local.is_valid(&value));
        assert!(decode_native(&to_vec(&value).unwrap()).is_err());
        assert!(serde_json::from_value::<LocalResponse>(value).is_err());
    }
    let mut mixed = result();
    mixed["hasMore"] = json!(true);
    assert!(decode_native(&to_vec(&mixed).unwrap()).is_err());
    assert!(serde_json::from_value::<LocalResponse>(mixed).is_err());
    let v1_shape = json!({"v":2,"type":"search-result","requestId":"r","items":[],"hasMore":false});
    assert!(!native.is_valid(&v1_shape) && !local.is_valid(&v1_shape));
    assert!(decode_native(&to_vec(&v1_shape).unwrap()).is_err());
    assert!(serde_json::from_value::<LocalResponse>(v1_shape).is_err());

    let partial = json!({"v":2,"type":"search-result","requestId":"r",
        "items":[],"nextCursor":"more","partialSources":["body"]});
    native.validate(&partial).unwrap();
    local.validate(&partial).unwrap();
    let message = decode_native(&to_vec(&partial).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), partial);
    let LocalResponse::SearchV2(reply) = serde_json::from_value(partial).unwrap() else {
        panic!("expected a v2 result");
    };
    reply.validate("r", 1).unwrap();
    assert!(reply.validate("unrelated", 1).is_err());
}

#[test]
fn cursors_use_utf16_limits_and_body_snippets_use_scalars() {
    for (count, accepted) in [(1_000, true), (1_001, false)] {
        let mut value = search();
        value["cursor"] = json!("😀".repeat(count));
        assert_eq!(decode_native(&to_vec(&value).unwrap()).is_ok(), accepted);
        value["profileId"] = json!(PROFILE);
        let request = serde_json::from_value::<LocalRequest>(value).unwrap();
        assert_eq!(request.validate().is_ok(), accepted);
    }
    for (count, accepted) in [(160, true), (161, false)] {
        let mut value = result();
        value["items"][0]["snippet"] = json!("😀".repeat(count));
        assert_eq!(decode_native(&to_vec(&value).unwrap()).is_ok(), accepted);
    }
    for evidence in [
        json!(["title"]),
        json!(["body", "body"]),
        json!(["history"]),
    ] {
        let mut value = result();
        value["items"][0]["matchedIn"] = evidence;
        assert!(decode_native(&to_vec(&value).unwrap()).is_err());
    }
    let mut value = result();
    value["items"][0]["snippet"] = Value::Null;
    assert!(decode_native(&to_vec(&value).unwrap()).is_err());
}
