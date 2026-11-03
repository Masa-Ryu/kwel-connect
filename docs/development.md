# Development

## Workspace

| Crate | Responsibility |
| --- | --- |
| `kwel-protocol` | Native v1 and local v1 types, validation and framing |
| `kwel-native-host` | bounded async sessions, secure IPC, OS registration and installers |
| `kwel-client` | language-neutral JSON CLI and public protocol client |
| `kwel-alfred` | official Alfred JSON, connection retry and Chrome activation |

Use the Rust toolchain pinned by `rust-toolchain.toml`. Cargo.lock is committed. Tokio is used for bounded asynchronous Named Pipe/Unix socket I/O, cancellation and timeouts; no unbounded per-connection thread model is used. Windows FFI is isolated.

```sh
bash scripts/verify.sh
python3 -m unittest discover -s scripts/tests -v
cargo build -p kwel-protocol --example native-conformance --locked
node scripts/test-kwel-parser.cjs /path/to/current/kwel-checkout
node --test scripts/tests/chrome-e2e.test.mjs
```

Tests use synthetic IDs and isolated directories. `scripts/test-installer.py` additionally exercises actual package install/upgrade/removal only on disposable GitHub-hosted runners; it refuses ordinary local environments. Do not run installer tests against a real user home.

Protocol compatibility is a behavioral result, not equality of repository commits or document hashes. `scripts/test-kwel-parser.cjs` reads Connect's canonical Native65 directly and runs the real TypeScript/Zod parser/builders plus actual Rust frames and 12 Unicode boundaries. It logs both checkout SHAs as evidence, without reading Kwel pin metadata, requiring latest HEADs, or comparing Markdown/Schema hashes. There is no separate local-review bypass mode.

Kwel's ordinary protocol tests are offline and independent. `pnpm test:local-protocol:pair` runs Vitest conformance tests against Connect's Native65 corpus and Rust adapter, including Unicode boundaries and URL acceptance/projection. These tests use `KWEL_CONNECT_ROOT` as the filesystem path to the Connect checkout; CI passes its resolved checkout path to the same tests. This is not a CI source-version selector.

Real Chrome E2E runs separately in the same **Native v1 pair** CI job via `scripts/test-native-chrome.mjs`. It checks Chrome-launched Host communication, search ranking/URLs, empty searches, profile isolation and page opening with single-use openRefs.

Kwel's required n100 **Native v1 pair** job resolves Connect once per job in **Resolve latest Connect source**: while Connect PR #1 is open, it selects that PR's latest HEAD; after the PR is merged, it selects the latest `main` HEAD. An unmerged closed PR, API error, invalid full SHA or failed commit confirmation fails closed. The job records the resolved SHA, confirms the checkout matches it, and uses that same Connect checkout for all paired Rust/parser tests, Host builds and real Chrome verification.

Selecting the latest HEAD determines what is tested; Protocol v1 compatibility is still behavioral conformance, not equality of Connect/Kwel SHAs or document hashes. A developer may manually choose a specific commit or tag for local verification and record both actual SHAs; that does not override automatic CI selection. Public Connect CI receives neither private source nor cross-repository credentials.

For real Chrome-launched Host, two-profile history/ranking/openRef and OS approval checks, use [the E2E verifier and manual sign-off procedure](chrome-e2e.md). Node/CDP guard tests, parser tests and the installed synthetic-peer test are not real Chrome/Alfred E2E and must never be reported as such.

macOS builds need both Darwin Rust targets, Xcode's `lipo`, `pkgbuild`, `productbuild`, the pinned LLVM tools component, and **resvg 0.45.1** (`cargo install resvg --version 0.45.1 --locked`). Windows packaging needs NSIS 3.11. Ubuntu packaging uses `dpkg-deb`. Install the audit tool with `cargo install cargo-audit --version 0.22.2 --locked`. These are development tools; end users install prebuilt packages.

macOS builds all three executables for arm64/x86_64 and creates Universal binaries. Windows release builds explicitly target x64 MSVC with static CRT; package verification rejects references to separately installed Visual C++ runtime DLLs. Ubuntu debs declare their libc/libgcc dependencies. Alfred packages retain sorted ZIP entries, fixed timestamps, explicit Unix modes, the single tracked SVG source and required empty Profile ID configuration. Installer inputs are selected explicitly and never include prefs, caches, tokens or user data.

## Runner and distribution compatibility

Release producers are macOS 26 arm64, Windows Server 2025 x64, and Ubuntu 24.04 amd64/arm64. The macOS 26 Intel job also builds/tests both Universal slices but does not upload a release artifact. Rust 1.99.0, resvg 0.45.1, cargo-audit 0.22.2, NSIS 3.11 and the Action SHAs remain fixed.

Each Darwin cross-build explicitly sets `MACOSX_DEPLOYMENT_TARGET`: **10.12 for x86_64**, **11.0 for arm64**, retaining the pre-update binary settings. `lipo` checks both slices; `vtool` checks their actual load-command minimum, including the binaries inside pkg and Workflow. The product's documented macOS 13+ requirement follows current Chrome support; older binary deployment targets alone do not establish Kwel/Chrome compatibility on obsolete OS versions. macOS 15 arm64/Intel jobs install the original macOS 26 arm64 producer's Universal pkg and execute its Host, JSON CLI and Workflow search binary. They do not rebuild those packages. macOS 13/14 runtime and real Chrome/Alfred UI remain manual checks.

Linux release debs are built only on Ubuntu 24.04 and keep `Depends: libc6 (>= 2.39), libgcc-s1`. Verification extracts the actual deb payload/control archives and inspects Native Host, JSON CLI, preinst and prerm with readelf. Required numeric GLIBC symbol versions must not exceed 2.39; the known DT_RELR ABI marker is supported since 2.36. Unknown/private ABI requirements and undeclared shared-library dependencies fail verification.

Ubuntu 26.04 amd64/arm64 jobs run source tests/builds separately, then download the corresponding **24.04-produced deb**, verify its producer/hash/ELF metadata, and install, communicate, reinstall and remove that unchanged file. No Ubuntu 26.04 build is uploaded or used for release. `BUILDINFO.json` records only product version, OS/architecture and package hashes. Collection checks producer roles and all hashes before assembling the same five distribution names and SHA256SUMS; build provenance is not an extra Release asset.

Release producer artifacts are always transferred within the run to support these consumer tests, with one-day retention by default or two days for release/review runs. Compatibility consumers never upload release artifacts. The release collector's own runner can be Ubuntu 26.04 because it copies validated files without rebuilding Linux binaries.

Automatic Test runs on pull requests and pushes to main; development-branch pushes do not start a second full run. Manual dispatch and reusable calls remain available. Concurrency includes the caller workflow, event, repository and PR number (or branch), so only older runs of the same PR are superseded and manual Release validation remains independent. GitHub documents these [concurrency groups](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency).

actionlint 1.7.12's bundled runner catalog does not yet include the two [official Ubuntu 26.04 labels](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). For that version, validate syntax with `actionlint -ignore '^label "ubuntu-26\.04(-arm)?" is unknown\.' .github/workflows/test.yml .github/workflows/release.yml`. Only those catalog false positives are excluded; actual hosted execution and installer/security checks remain required. No self-hosted label aliases are added.

## First release and development installations

Kwel Connect 1.0.0, implemented in Rust, is the first formal release. Before removing development-only Go
compatibility, the GitHub API was checked on 2026-10-26: no tags, Releases,
Release assets or Release workflow runs existed. The old README described local
builds only. This establishes the repository's visible distribution history,
not the absence of private developer copies.

The display version `1.00` maps to SemVer `1.0.0`; release tags are `v1.0.0`
and later. No historical tags are renamed or deleted. Release preflight requires
a main commit and matching Cargo/config/Workflow version; publication refuses
existing Releases. The repository was renamed in place, retaining history,
Issue #2 and PR #1. `com.kwel.alfred.search` remains the Alfred bundle ID.

Developers with an old local Host must follow the [one-time manual replacement
procedure](development-host-replacement.md). The product registers only
`com.kwel.bridge`; there is no old-Host socket discovery, fallback, coordination
or registration rewrite.

## Manual release

The next patch is prepared as **1.0.2** so the distributed Alfred Workflow
includes the macOS first-launch warning and SHA-256 checksum guidance merged
in PR #5. The v1.0.1 release introduced the Alfred Profile ID mismatch diagnostic
in PR #4. Local Client Protocol v1 and the existing Native Host origin remain
unchanged. Version preparation does not publish a Release; the existing
v1.0.0 and v1.0.1 tags and Releases must remain untouched.

Only the repository owner's explicit **Actions → Release → Run workflow**
starts Release. Merging a PR, pushing main, or pushing a tag never starts it.
The Test workflow keeps its existing automatic triggers. The Release workflow
must first be merged into the default branch to appear in the Actions UI.

1. Prepare and review main with matching Cargo, distribution configuration and
   Alfred Workflow versions. Check `python3 scripts/check-release-version.py v1.0.2`
   from that checkout, substituting the intended version.
2. Create and push the matching tag yourself at the current main HEAD. For example,
   after fetching main, inspect `git rev-parse origin/main`, then run
   `git tag v1.0.2 <that-full-SHA>` and `git push origin refs/tags/v1.0.2`.
   If the correct tag already exists, reuse it. If it points elsewhere, stop;
   do not force, move or overwrite it. Prepare a new version/tag when needed.
3. Open **Actions → Release → Run workflow**, select branch **main**, enter
   the existing tag (for example `v1.0.2`), and press **Run workflow**. This is
   the publication request; there is no second approval screen. After all
   validations succeed, the workflow publishes the five installers/Workflow
   files and `SHA256SUMS` automatically.

Preflight requires `workflow_dispatch`, main, an existing commit tag exactly
matching the dispatch's `github.sha`, matching SemVer/product versions, main
ancestry and no existing Release (including drafts). API/authentication/network
errors fail closed; only HTTP404 means no Release. Lightweight and annotated
tags are supported. No script creates or moves tags.

The dispatch SHA is main's HEAD **at dispatch time**. Every checkout and the
reusable Test workflow use that immutable snapshot. Main can advance afterwards
without changing the selected artifacts, as long as the snapshot remains in its
history. Publication fetches fresh refs and repeats preflight so a moved tag or
newly created Release is rejected before upload. It uses only this run's
validated artifacts, verifies checksums and stages a draft before making it
public. No Linux package is rebuilt by publish; only publish has write permission.

If validation fails, no publication job runs. An upload/publication failure can
leave an unpublished draft for the owner to inspect; retries reject that existing
Release rather than overwriting it. Inspect or resolve it manually before retrying.
Actual Release publication is not exercised by repository tests: they use
isolated Git fixtures, mocked read-only GitHub responses and workflow policy checks.

## Shared Native v1 release gate

`cargo test --workspace --locked` includes Native65, Schema and current validation boundaries, secure IPC/session and installation regressions. `python3 scripts/check-native-fixture.py` verifies Connect's canonical owner/path and SHA-256 without another checkout. Adoption source fields are read-only history; no partner files, commits or document hashes are compared.

Connect's canonical-corpus job and all OS Rust tests remain required by Release's reusable `validate`. Kwel's required **Native v1 pair** runs on n100, reads the selected checkout directly without a copied corpus or document hashes, tests both implementations, builds the real Host/CLI and production extension, and runs Google Chrome in two disposable profiles. Failed, cancelled, skipped or missing pair results block Kwel's `Verify` and automatic Release. Its ordinary application checks and full local pre-push gate remain required. PR permissions stay read-only; no pair secrets or artifact uploads are added.

Before a formal Connect release, also review the matching successful private pair run and perform the OS/GUI checks in [the E2E procedure](chrome-e2e.md). Connect cannot query a private Kwel run using its public PR token; its green CI alone does not attest that private run or approve a release. No release is created by these protocol changes.

Unix installer security tests execute each fixture in a dedicated subprocess so another test's fork cannot briefly inherit its live file lock. Every original assertion still runs. Production locking and concurrent IPC tests remain unchanged; no retry, sleep or skip hides a lock failure.

The shared corpus is checked out with LF on every OS via `.gitattributes`; Windows `core.autocrlf` must not change the bytes whose SHA-256 is recorded. Markdown example checks accept LF/CRLF, validate every JSON block and require all v1 message types rather than a fixed example count.

## Source references and compatibility

| Reference | Role |
| --- | --- |
| Kwel CI's **Resolve latest Connect source** | Automatic per-job selection: open PR #1's latest HEAD, or latest `main` HEAD after merge. Records the resolved SHA and reuses the same checkout throughout the job; invalid/failed resolution fails closed. |
| Local pair tests' `KWEL_CONNECT_ROOT` | Filesystem path to a Connect checkout, also set by CI to its resolved checkout path. Not a CI version-selection variable. |
| Connect's `crates/kwel-protocol/tests/fixtures/native-messaging-v1.source.json` | Historical Kwel adoption origin and corpus integrity. No current-Kwel tracking requirement. |
| Pair logs' Connect/Kwel SHAs | Evidence of the actual two implementations tested; not compared to each other or to document hashes. |

The Release tag/dispatch SHA separately fixes Connect's publication snapshot. None of these references is a runtime Protocol field or dependency. Documentation-only edits do not change protocol compatibility.
