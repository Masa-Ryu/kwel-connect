# Native Messaging Protocol v2

Chrome's Kwel extension connects to the registered `com.kwel.bridge` Host. The [Native v2 schema](schemas/native-messaging-v2.schema.json) defines the closed message shapes. Origin checks, framing, URL validation, Unicode, request correlation, timeouts and openRef lifetime follow [Native v1](native-messaging-v1.md).

## Version selection

The first `hello.v` selects the contract for the lifetime of the Port. The Host replies with `hello-ack` at that version and uses it for every subsequent frame. A version mismatch on an established Port ends that session. Each profile is tied to a single active Native connection.

The Host supports `v: 1` and `v: 2` on the same registration. With a legacy Host, a v2-capable extension can retry the initial handshake once on a fresh Port after a recognized compatibility failure. Search and open each run once per client request. Kwel manages sharing consent for page text and Chrome bookmarks in each profile.

## Search

`hello`, `hello-ack`, `ping`, `pong`, `open-page`, `open-result` and `error` use the v1 fields with `v: 2`. A v2 `search` adds optional `cursor` and `includeSnippet`. The query limit remains 200 UTF-16 units, the page size 1–100, and the Native default 20. Cursors are opaque strings of at most 2,000 UTF-16 units. Pass a returned cursor back to the same profile, search and Port. Kwel validates cursor scope and freshness. Omit it to begin a new search.

```json
{"v":2,"type":"search","requestId":"s","query":"Kyoto","limit":20,"includeSnippet":true}
```

```json
{"v":2,"type":"search-result","requestId":"s","items":[{"title":"Travel notes","url":"https://example.com/notes","domain":"example.com","openRef":"00000000-0000-4000-8000-000000000002","matchedIn":["body"],"snippet":"Kyoto was our first stop."}],"nextCursor":null,"partialSources":[]}
```

- Each item carries the existing title, URL, domain and opaque UUID openRef. `matchedIn` lists unique fields from `title`, `url`, `bookmark` and `body`. An empty query can return an empty match list.
- `snippet` is optional plain text of up to 160 Unicode scalar values from a body match, available only with `includeSnippet: true` and Kwel's page-text sharing authorization. Treat it as text when rendering.
- `nextCursor` is a continuation string or `null`. `partialSources` lists any incomplete sources from `history`, `bookmark` and `body`. A partial result may contain zero items and a continuation.
- Kwel owns the ranking, paging and 1,000,000-byte response budget. It first omits snippets when needed, then limits the page and issues a cursor for the items returned. The Host reserves space for restoring the caller's request ID and relays the v2 page in the received order.

Opening a page uses a reference issued by search and checked against the current Kwel connection and page state. A reference is single-use, expires after at most five minutes, and belongs to the selected profile and session.
