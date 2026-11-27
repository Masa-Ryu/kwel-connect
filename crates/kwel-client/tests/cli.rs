use kwel_client::{Operation, parse_args};
use kwel_protocol::LocalRequest;
fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}
#[test]
fn json_cli_preserves_raw_arguments_and_normalizes_profile() {
    for query in ["Kyoto", "日本語 ⚙", "quotes '\" $() ! ;", "--limit"] {
        let args = strings(&[
            "search",
            "--profile-id",
            " 00000000-0000-4000-8000-00000000000A ",
            "--query",
            query,
            "--format",
            "json",
        ]);
        assert!(
            matches!(parse_args(&args,"json").unwrap(),Operation::Request(LocalRequest::Search{query:actual,profile_id,limit:20,..}) if actual==query&&profile_id=="00000000-0000-4000-8000-00000000000a")
        );
    }
}
#[test]
fn invalid_cli_never_becomes_a_management_or_url_open_request() {
    for args in [
        strings(&["doctor"]),
        strings(&["uninstall"]),
        strings(&["search", "--query", "Kyoto"]),
        strings(&[
            "search",
            "--profile-id",
            "00000000-0000-4000-8000-00000000000a",
            "--format",
            "alfred",
        ]),
        strings(&[
            "open",
            "--profile-id",
            "00000000-0000-4000-8000-00000000000a",
            "--ref",
            "https://example.com",
        ]),
    ] {
        assert!(parse_args(&args, "json").is_err());
    }
}

#[test]
fn rich_search_requires_explicit_json_protocol_selection() {
    let args = [
        "search",
        "--profile-id",
        "00000000-0000-4000-8000-00000000000a",
        "--query",
        "本文",
        "--protocol",
        "2",
        "--cursor",
        "--opaque",
        "--include-snippet",
        "true",
    ];
    let Operation::Request(LocalRequest::SearchV2(request)) =
        parse_args(&strings(&args), "json").unwrap()
    else {
        panic!("expected a Local v2 search");
    };
    assert_eq!(request.query, "本文");
    assert_eq!(request.cursor.as_deref(), Some("--opaque"));
    assert_eq!(request.include_snippet, Some(true));
    assert_eq!(request.limit, 20);
    assert!(parse_args(&strings(&args), "alfred").is_err());
    let mut v1 = args;
    v1[6] = "1";
    assert!(parse_args(&strings(&v1), "json").is_err());
    let mut invalid = args;
    invalid[10] = "yes";
    assert!(parse_args(&strings(&invalid), "json").is_err());
}
