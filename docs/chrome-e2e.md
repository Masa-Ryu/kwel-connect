# Real Google Chrome E2E

This check uses the real Kwel extension, Chrome-launched installed Host, public JSON CLI and actual Chrome history/tabs. It never fabricates a Native Port or returns synthetic search results. `scripts/test-installer.py` remains a **separate synthetic-peer test** of installed packages. Parser conformance is also separate; neither proves this E2E passed.

## Preparation and consent

Use a disposable OS account/VM for installer and GUI checks. Never use a personal Chrome profile or real history exports. Keep the OS sandbox, Gatekeeper, SmartScreen and Chrome permissions enabled. Record a blocked dialog rather than bypassing it. The script does not grant optional permissions or change Kwel consent/storage.

1. Install the package produced by the exact Kwel Connect commit under review through the ordinary OS installer. Record package SHA256 and Host version. For Windows, confirm the HKCU manifest points at that installed Host; for Ubuntu use the real deb, not a source build. For macOS use the pkg first, then the Workflow.
2. Use Google Chrome and a reviewed Kwel build with **Local Connection**, from the exact PR #211 commit being tested. Record source commit plus hashes of the actual manifest/service-worker/UI/Wasm build. A Store build without Local Connection is not this counterpart. An unpacked review build must use the published extension identity `mhablbmgafbdofgaffhdnnknjhafmlin` (public manifest key); do not expand Host allowed_origins to accommodate another ID. Record unpacked versus Store installation provenance.
3. Developer verification requires Node.js 24+; end users still do not need Node or Rust. Run `node scripts/test-chrome-e2e.mjs prepare`. It creates two empty temporary user-data directories with ownership markers and prints the plan path and A/B paths. Only these directories can be attached by the verifier.
4. Launch **two Google Chrome processes**, using the printed A/B directories, distinct loopback debugging ports, `--enable-automation` and `--no-first-run`. Example flags for A: `--user-data-dir="<A>" --remote-debugging-address=127.0.0.1 --remote-debugging-port=9222 --enable-automation --no-first-run`. Use port 9223 and directory B for the other. On macOS also pass **`--use-mock-keychain`**. Do not attach the normal Chrome process.
5. Install/open Kwel separately in each disposable Chrome. Open Settings, enable **Local Connection**, read its sharing disclosure and accept any Chrome permission prompt explicitly. Leave the two Settings tabs open and wait for **Connected**. Connected is set by Kwel only after hello/hello-ack; the verifier additionally requires a live public profile-probe/ping/pong.

With a custom user-data directory, macOS/Linux Chrome looks for user-level manifests in that directory's `NativeMessagingHosts/`. Copy the **installed** `com.kwel.bridge.json` there without changing its executable path or allowed_origins. This is test-browser registration, not evidence that the normal installer writes to arbitrary custom profiles. Windows continues using HKCU. See [Chrome's manifest locations](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging) and [Chromium's user-data path resolution](https://chromium.googlesource.com/chromium/src/+/HEAD/chrome/common/chrome_paths.cc).

## OS-specific items

| OS | Installer / installed JSON CLI | Additional manual evidence |
| --- | --- | --- |
| macOS Apple Silicon / Intel | `KwelConnect-macos-universal.pkg`; `~/Library/Application Support/Kwel/NativeBridge/bin/kwel-bridge` | pkg approval, Gatekeeper, Chrome permission prompt; import `Kwel.alfredworkflow`, configure A's Profile ID, register Search Kwel in Fallback Search |
| Windows x64 | `KwelConnect-windows-x64-setup.exe`; `%LOCALAPPDATA%\Kwel\NativeBridge\bin\kwel-bridge.exe` | SmartScreen/installer approval; HKCU registration; Chrome optional permission prompt |
| Ubuntu amd64 / arm64 | matching `KwelConnect-ubuntu-*.deb`; `/usr/bin/kwel-bridge` | package manager approval, official Google Chrome; current user runtime/IPC permissions |

Do not disable Local Connection in unrelated profiles or remove a shared Host that another launcher uses. Installer upgrades/removal still require all users of that Host to disconnect through their own settings. Review ownership/in-use failures rather than deleting locks or manifests manually.

## Execute the real transport check

From this repository, using the plan and installed CLI paths above:

```sh
node scripts/test-chrome-e2e.mjs \
  --plan "<temporary plan.json>" \
  --a http://127.0.0.1:9222 --b http://127.0.0.1:9223 \
  --cli "<installed kwel-bridge executable>" \
  --out "<new private evidence.json>"
```

The verifier refuses unmarked/personal profiles, substituted browser executables and missing consent/connection state. It serves local synthetic pages, visits two distinct URLs per profile and one shared URL, and waits on Kwel state-change events for indexing. No fixed sleep, injected Native response, direct database edit or replacement search implementation is used.

It compares the public CLI's ordered title/URL/domain list to the **actual bundled Kwel UI's SEARCH_PAGES response**, checks empty-query acceptance, checks that the other profile's history is absent, rejects cross-profile openRef use, and opens the shared fixture that is already present in both profiles. It requires the same CDP target and Chrome tab ID in the selected profile, an active tab and focused window, no new targets, and unchanged in-memory/document input state (no reload or navigation). It then closes only that fixture in the selected profile, obtains a fresh opaque reference from real search, and requires exactly one new target there. Both focus and creation consume their references: reuse must return `RESULT_EXPIRED`. The other profile's target IDs, URLs and active tabs must remain unchanged throughout. The report contains OS/Chrome versions and pass flags; no Profile IDs, queries, results or URLs are written. A successful run proves only that exact installed build/browser/OS combination.

The verifier closes only the fixture tabs it created and its local server. Close the two test Chrome processes and remove their allocated temporary profiles afterwards. Preserve evidence outside the profile directories. Uninstall only in the disposable account, after disabling Local Connection there; do not auto-remove another launcher's Host.

## macOS Alfred UI and lifecycle sign-off

These are **manual checks**, not attested by a passing JSON CLI/CDP run:

1. In Alfred type a query matching an indexed fixture page, select **Search Kwel** without a keyword/hotkey, and compare the order/URLs with Kwel. Press Return with a matching page already open in A and verify that exact tab is selected without reloading or duplicating it. Close that test tab, search again, and verify Return creates one new tab in A. Repeat for B after changing only Workflow Configuration's Profile ID.
2. Query a phrase missing from A but present in B; ensure no B result appears while A is configured. Check the first connection's automatic retry and one disabled error row on disconnection.
3. Reinstall/update the same reviewed package after disconnecting both profiles, reconnect and rerun. Record separately whether a real **different-version** upgrade was tested. Disable both test connections, remove the Host through its normal uninstaller, and verify Chrome reports Host not found without affecting the other user's data.
4. Record Chrome's native permission dialogs, Gatekeeper/SmartScreen and installer approvals as **observed / blocked / not run**. Never infer approval behavior from a command-line installation.

## Evidence status

Automated package CI covers install/reinstall/removal with a synthetic Native peer and OS IPC/registration tests. The separate source-built real Chrome pair check below runs in private Kwel CI. **Full installed-package Chrome E2E and Alfred UI on all supported OSes still require manual sign-off.** The verifier and procedure are provided for those runs; an absent evidence file or a failed prerequisite is not a pass. Record exact Connect SHA, Kwel SHA/build digest, package hash, browser version, OS/architecture, script result and the manual items above before release.

## Automated source-built pair check

Kwel's required **Native v1 pair** job runs on the Linux n100 runner. **Resolve latest Connect source** automatically selects Connect PR #1's latest HEAD while open, or the latest `main` HEAD after it is merged. An unmerged closed PR, API error, invalid full SHA or failed commit confirmation fails closed. The job records the resolved Connect SHA and Kwel SHA, confirms the Connect checkout matches the resolved SHA, and uses that same checkout for every paired Rust/parser test, Host build and Chrome verification. Selecting the latest HEAD determines the source under test; Protocol v1 compatibility is behavioral conformance and does not require equal Connect/Kwel SHAs.

For manual verification, choose the particular Connect commit/build under review and record both SHAs as described above; this differs from automatic CI selection. `KWEL_CONNECT_ROOT` is the Connect checkout path consumed by local pair tests, including those run in CI, not a CI source-version selector.

The CI job builds the selected Rust source and current production extension, then invokes:

```sh
node /path/to/kwel-connect/scripts/test-native-chrome.mjs \
  --chrome /path/to/google-chrome --kwel-dist /path/to/kwel/dist \
  --host /path/to/kwel-native-host --cli /path/to/kwel-bridge \
  --out /new/private/path/evidence.json
```

This macOS/Linux launcher owns and removes two fresh browser profiles. It uses the published **public** manifest key to preserve the existing exact origin allowlist, and pregrants `nativeMessaging` only in its disposable unpacked manifest. It enables Local Connection with real Settings clicks; no consent storage or Native frames are fabricated. The real verifier waits for history/model events and asserts an unchanged model revision around its exact ordered title/URL/domain comparison. It checks empty search, cross-profile rejection, existing-tab focus without reload or duplication, exactly one new tab when no match exists, and single use after both outcomes. The shared URL is open in both profiles, so the test also verifies that the other profile's tabs remain unchanged. Fixture history is local HTTP data only. The safe summary records source commits, source cleanliness, binary/bundle hashes, browser version and checks; no Profile IDs, queries or browsing URLs.

This proves real Chrome-launched **source-built** Host communication, not OS installer operation, Store installation, optional permission prompts, Gatekeeper/SmartScreen or Alfred UI. CI requires clean tracked sources. A dirty local debugging run is diagnostic only. The installer/synthetic tests and manual table above remain necessary and separately reported. Windows still uses the installed-Host manual verifier. Before a formal release, record the actual successful paired run plus outstanding OS/manual results; a missing or failed evidence file is never a pass.

## Coordinated navigation change

[Masa-Ryu/kwel#227](https://github.com/Masa-Ryu/kwel/issues/227) changes ordinary Kwel UI activation and Native `open-page` to prefer an existing tab in the connected profile. Native Messaging v1 and Local Client Protocol v1 stay unchanged: clients send only `openRef` and receive the existing `open-result { ok: true }`. Tab IDs and the focus/create outcome are never returned to the Host or CLI.

This verifier deliberately rejects the former always-create behavior. The Kwel pair job continues resolving the latest Connect main/PR #1 under its existing rules; it must consume this updated verifier before the Kwel navigation PR can pass its required pair check. Record both source SHAs for a local paired-branch run; that run does not attest the automatically selected CI pair or an eventual merge SHA.
