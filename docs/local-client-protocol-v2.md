# Local Client Protocol v2

Local clients use the existing OS-user-owned IPC endpoint. Apply the [Local v1 transport, endpoint and identity rules](local-client-protocol-v1.md) to v2. Each connection carries one request, limited to 16,384 UTF-8 bytes, and one response, limited to 1,000,000 UTF-8 bytes. See the [Local v2 schema](schemas/local-client-v2.schema.json). For offline use, register the [adjacent Native v2 schema](schemas/native-messaging-v2.schema.json) under its `$id` to resolve the shared fields.

Every request uses `v: 2`, a nonempty `requestId` of at most 100 UTF-16 units and the target `profileId`. Responses to valid requests use the requested Local version and echo the request ID. Probe and open use the v1 fields with `v: 2`. Search requires `query` and `limit` and accepts optional `cursor` and `includeSnippet`. The [Native v2 result fields](native-messaging-v2.md#search), including the continuation and partial sources, are carried through to Local v2.

```json
{"v":2,"type":"search","requestId":"s","profileId":"00000000-0000-4000-8000-000000000001","query":"Kyoto","limit":20,"includeSnippet":true}
```

```json
{"v":2,"type":"search-result","requestId":"s","items":[{"title":"Travel notes","url":"https://example.com/notes","domain":"example.com","openRef":"00000000-0000-4000-8000-000000000002","matchedIn":["body"],"snippet":"Kyoto was our first stop."}],"nextCursor":null,"partialSources":[]}
```

## Version interoperability

Local v2 requires an active Native v2 session. An older extension or a profile still using its legacy sharing grant can have a Native v1 session. The Host returns a v2 `INCOMPATIBLE_VERSION` error for a v2 request to that session. For page-text sharing, review the scope in Kwel Settings and enable Local Connection with the updated grant. Local v1 clients can use either Native version. On Native v2, the Host returns the v1 item fields and sets `hasMore` when Kwel reports a continuation or a partial source. An empty result with incomplete history or bookmarks returns `SEARCH_FAILED`, matching Kwel's Native v1 behavior.

To detect an older Host, use a v2 `profile-probe`. A v1-only Host may return a v1 `INCOMPATIBLE_VERSION` error with the probe ID. This is a compatibility signal. An older decoder can return a generic invalid-request error with an empty ID for a search carrying v2-only fields. The Rust client confirms that case using a separate probe. Each client can explicitly issue a fresh v1 request when it accepts the v1 contract.

## JSON CLI

`kwel-bridge` defaults to v1. Use `--protocol 2` on `profile-probe`, `search` or `open` to request v2. `profiles` continues to discover both kinds of sessions through v1.

```sh
kwel-bridge search --profile-id "$KWEL_PROFILE_ID" --protocol 2 --query Kyoto --limit 20 --include-snippet true
kwel-bridge search --profile-id "$KWEL_PROFILE_ID" --protocol 2 --query Kyoto --limit 20 --include-snippet true --cursor "$NEXT_CURSOR"
kwel-bridge open --profile-id "$KWEL_PROFILE_ID" --protocol 2 --ref "$OPEN_REF"
```

Use the returned `nextCursor` as `NEXT_CURSOR` when it is a string. Keep the snippet setting the same while using that cursor. Result titles, URLs, snippets and cursors can come from private browsing content shared with the launcher by the user. Keep them local and avoid writing them to diagnostics.
