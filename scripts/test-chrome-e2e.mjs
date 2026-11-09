#!/usr/bin/env node
// Real Chrome + installed Host only. Never sends synthetic Native frames,
// grants permissions, changes consent, or connects to personal Chrome profiles.
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { execFile } from 'node:child_process';
import { createServer } from 'node:http';
import { mkdtempSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { promisify } from 'node:util';
import { pathToFileURL } from 'node:url';

const execute = promisify(execFile);
const extension = JSON.parse(readFileSync(new URL('../config/distribution.json', import.meta.url))).chromeExtensionId;
const marker = '.kwel-connect-e2e.json';
const flags = process.argv.slice(2);
let phase = 'preflight';
function stage(value) { phase = value; console.log(`Chrome E2E phase: ${value}`); }
function option(name) { const index = flags.indexOf(name); assert.ok(index >= 0 && flags[index + 1], `Missing ${name}`); return flags[index + 1]; }
function loopbackEndpoint(value) {
  const url = new URL(value);
  assert.ok(url.protocol === 'http:' && ['127.0.0.1', '[::1]'].includes(url.hostname) && !url.username && !url.password && url.pathname === '/', 'CDP must be a loopback test endpoint.');
  return url;
}

export class CDP {
  constructor(socket) {
    this.socket = socket; this.sequence = 0; this.pending = new Map(); this.listeners = new Set();
    socket.addEventListener('message', event => {
      if (typeof event.data !== 'string' || event.data.length > 1_000_000) { socket.close(); return; }
      let value;
      try { value = JSON.parse(event.data); } catch { socket.close(); return; }
      if (value.id) {
        const request = this.pending.get(value.id);
        if (!request) return;
        this.pending.delete(value.id); clearTimeout(request.timer);
        if (value.error) request.reject(new Error('Chrome rejected an E2E command.'));
        else request.resolve(value.result);
      } else for (const listener of this.listeners) listener(value);
    });
    socket.addEventListener('close', () => {
      for (const request of this.pending.values()) { clearTimeout(request.timer); request.reject(new Error('Test Chrome disconnected.')); }
      this.pending.clear();
    });
  }
  static async connect(endpoint) {
    const url = loopbackEndpoint(endpoint);
    const response = await fetch(new URL('/json/version', url), { signal: AbortSignal.timeout(5000) });
    const version = await response.json();
    const address = new URL(version.webSocketDebuggerUrl);
    assert.ok(['127.0.0.1', '[::1]'].includes(address.hostname), 'Chrome returned a non-loopback endpoint.');
    const socket = new WebSocket(address);
    await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', () => reject(new Error('Could not connect to test Chrome.')), { once: true }); });
    return new CDP(socket);
  }
  call(method, params = {}, sessionId) {
    assert.ok(this.pending.size < 16, 'Too many E2E browser commands.');
    const id = ++this.sequence;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Chrome E2E command timed out: ${method}`)); }, 50_000);
      this.pending.set(id, { resolve, reject, timer });
      this.socket.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }));
    });
  }
  event(predicate) {
    let listener;
    const promise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => { this.listeners.delete(listener); reject(new Error('Expected Chrome event did not arrive.')); }, 15_000);
      listener = value => { if (predicate(value)) { clearTimeout(timer); this.listeners.delete(listener); resolve(value); } };
      this.listeners.add(listener);
    });
    // The command following registration may fail before the event arrives.
    // Mark the promise handled while preserving rejection for its awaiter.
    promise.catch(() => {});
    return promise;
  }
  async evaluate(expression, session) {
    const value = await this.call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, session);
    assert.ok(!value.exceptionDetails && value.result.type !== 'undefined', 'Kwel test-page evaluation failed.');
    return value.result.value;
  }
  close() { this.socket.close(); }
}

// These assertions run against real profile-scoped Chrome APIs and CDP targets.
// An open-result alone cannot prove reuse, focus, state preservation or isolation.
export async function verifyPageOpening({ a, b, selected, query, command, created }) {
  const targets = async browser => (await browser.call('Target.getTargets')).targetInfos
    .filter(value => value.type === 'page').map(({ targetId, url }) => ({ targetId, url }))
    .sort((left, right) => left.targetId.localeCompare(right.targetId));
  const activeTabs = browser => browser.evaluate(`chrome.tabs.query({active:true}).then(tabs=>tabs.map(tab=>tab.id).sort((a,b)=>a-b))`, browser.session);
  const beforeA = await targets(a), beforeB = await targets(b), activeB = await activeTabs(b);
  const existing = beforeA.filter(value => value.url === selected.url);
  assert.equal(existing.length, 1, 'Expected exactly one existing fixture target in profile A.');
  assert.ok(beforeB.some(value => value.url === selected.url), 'The isolation fixture must also be open in profile B.');
  const target = existing[0];
  const session = (await a.call('Target.attachToTarget', { targetId: target.targetId, flatten: true })).sessionId;
  const state = randomUUID();
  await a.evaluate(`(()=>{globalThis.__kwelOpenState=${JSON.stringify(state)};const input=document.createElement('input');input.id='kwel-open-state';input.value=${JSON.stringify(state)};document.body.append(input);return true;})()`, session);
  const original = await a.evaluate(`chrome.tabs.query({}).then(tabs=>{const tab=tabs.find(tab=>tab.url===${JSON.stringify(selected.url)});return {id:tab.id,windowId:tab.windowId};})`, a.session);
  // Move off the fixture first: opening an already active tab proves no switch.
  await a.evaluate(`chrome.tabs.query({}).then(tabs=>chrome.tabs.update(tabs.find(tab=>tab.url===location.href).id,{active:true})).then(()=>true)`, a.session);
  stage('cross profile openRef rejection');
  await command(b.profile, ['open', '--ref', selected.openRef], 'RESULT_EXPIRED');
  stage('focus existing target in original profile');
  await command(a.profile, ['open', '--ref', selected.openRef]);
  assert.deepEqual(await targets(a), beforeA, 'Opening an existing page created, closed or navigated a target.');
  assert.ok(await a.evaluate(`globalThis.__kwelOpenState===${JSON.stringify(state)}&&document.getElementById('kwel-open-state')?.value===${JSON.stringify(state)}`, session), 'Opening an existing page reloaded or lost its document state.');
  assert.ok(await a.evaluate(`(async()=>{const tab=await chrome.tabs.get(${original.id}),window=await chrome.windows.get(${original.windowId});return tab.active&&tab.windowId===window.id&&window.focused;})()`, a.session), 'The existing tab and its window were not activated.');
  stage('reject openRef reused after focus');
  await command(a.profile, ['open', '--ref', selected.openRef], 'RESULT_EXPIRED');
  assert.deepEqual(await targets(a), beforeA, 'A reused reference changed profile A targets.');
  assert.deepEqual(await targets(b), beforeB, 'Opening in profile A changed profile B targets.');
  assert.deepEqual(await activeTabs(b), activeB, 'Opening in profile A activated a tab in profile B.');

  stage('open missing target in original profile');
  const closed = a.event(value => value.method === 'Target.targetDestroyed' && value.params.targetId === target.targetId);
  await a.call('Target.closeTarget', { targetId: target.targetId });
  await closed;
  const missing = await targets(a);
  assert.ok(!missing.some(value => value.url === selected.url), 'The no-match fixture is still open.');
  const fresh = (await command(a.profile, ['search', '--query', query, '--limit', '20'])).items.find(value => value.url === selected.url);
  assert.ok(fresh, 'The closed fixture is missing from actual Kwel search results.');
  const beforeIds = new Set(missing.map(value => value.targetId));
  const opened = a.event(value => ['Target.targetCreated', 'Target.targetInfoChanged'].includes(value.method) && value.params.targetInfo.url === selected.url && !beforeIds.has(value.params.targetInfo.targetId));
  await command(a.profile, ['open', '--ref', fresh.openRef]);
  const newTarget = (await opened).params.targetInfo;
  created.push([a, newTarget.targetId]);
  const afterCreate = await targets(a);
  assert.equal(afterCreate.length, missing.length + 1, 'A missing page must create exactly one target.');
  assert.equal(afterCreate.filter(value => value.url === selected.url).length, 1, 'Opening a missing page created duplicate targets.');
  assert.notEqual(newTarget.targetId, target.targetId, 'A missing page did not receive a new target.');
  stage('reject openRef reused after creation');
  await command(a.profile, ['open', '--ref', fresh.openRef], 'RESULT_EXPIRED');
  assert.deepEqual(await targets(a), afterCreate, 'A reused creation reference changed profile A targets.');
  assert.deepEqual(await targets(b), beforeB, 'Creating in profile A changed profile B targets.');
  assert.deepEqual(await activeTabs(b), activeB, 'Creating in profile A activated a tab in profile B.');
  return {
    existingTargetReusedWithDocumentStateAndWindowFocus: true,
    missingTargetCreatedExactlyOnce: true,
    crossProfileOpenRefRejectedAndOtherProfileTabsUnchanged: true,
    openRefSingleUseAfterFocusAndCreate: true,
  };
}

async function main() {
  if (flags[0] === 'prepare') {
    const root = mkdtempSync(path.join(tmpdir(), 'kwel-connect-e2e-'));
    const plan = { nonce: randomUUID(), profiles: {} };
    for (const role of ['A', 'B']) {
      const directory = path.join(root, role); mkdirSync(directory, { mode: 0o700 });
      writeFileSync(path.join(directory, marker), JSON.stringify({ nonce: plan.nonce, role }), { mode: 0o600 });
      plan.profiles[role] = directory;
    }
    writeFileSync(path.join(root, 'plan.json'), JSON.stringify(plan, null, 2), { mode: 0o600 });
    console.log(JSON.stringify({ plan: path.join(root, 'plan.json'), ...plan.profiles }, null, 2));
    return;
  }
  const plan = JSON.parse(readFileSync(option('--plan'), 'utf8'));
  assert.ok(/^[a-f0-9-]{36}$/iu.test(plan.nonce), 'Invalid test plan.');
  const cli = path.resolve(option('--cli'));
  const output = path.resolve(option('--out'));
  const endpointA = option('--a'), endpointB = option('--b');
  assert.ok(endpointA !== endpointB, 'Two separate Chrome test profiles are required.');
  loopbackEndpoint(endpointA); loopbackEndpoint(endpointB);
  const browsers = [], created = [];
  const query = 'kyoto' + randomUUID().slice(0, 8).split('').map(value => String.fromCharCode(97 + parseInt(value, 16))).join('');
  const server = createServer((request, response) => {
    if (!request.url.startsWith(`/${plan.nonce}/`)) { response.writeHead(404); response.end(); return; }
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' });
    // Keep the first onVisited fallback title and final document title equal.
    // This tests transport data preservation without a metadata-update race.
    const title = `http://${request.headers.host}${request.url}`;
    response.end(`<title>${title}</title><p>Disposable local history fixture.</p>`);
  });
  await new Promise((resolve, reject) => {
    server.once('error', () => reject(new Error('Could not start the local E2E fixture server.')));
    server.listen(0, '127.0.0.1', resolve);
  });
  // Chrome's onVisited can precede its title update. Put the ordinary query
  // token in both the URL and title, so ingestion never relies on that race.
  const base = `http://127.0.0.1:${server.address().port}/${plan.nonce}/${query}`;
  const evidence = { schema: 1, os: process.platform, time: new Date().toISOString(), chrome: [], profiles: 2, checks: {}, alfredUI: 'unverified', permissionDialogs: 'manual prerequisite' };
  async function command(profile, args, failureCode) {
    let result;
    try { result = await execute(cli, [...args, '--profile-id', profile, '--format', 'json'], { timeout: 20_000, maxBuffer: 1_000_000, windowsHide: true }); }
    catch (error) { assert.ok(failureCode && error.stdout, 'Installed JSON CLI failed.'); result = error; }
    const value = JSON.parse(result.stdout);
    assert.ok(!result.stderr && (failureCode ? value.code === failureCode : value.type !== 'error'), 'Unexpected JSON CLI response.');
    return value;
  }
  try {
    for (const [index, endpoint] of [endpointA, endpointB].entries()) {
      const role = ['A', 'B'][index];
      stage(`verify Chrome and Host ${role}`);
      const browser = await CDP.connect(endpoint); browsers.push(browser);
      const { arguments: arguments_ } = await browser.call('Browser.getBrowserCommandLine');
      const directory = arguments_.find(value => value.startsWith('--user-data-dir='))?.slice('--user-data-dir='.length);
      assert.ok(directory && path.resolve(directory) === path.resolve(plan.profiles[role]), 'Refusing a Chrome profile outside the E2E plan.');
      const owned = JSON.parse(readFileSync(path.join(directory, marker), 'utf8'));
      assert.ok(owned.nonce === plan.nonce && owned.role === role, 'Missing E2E profile ownership marker.');
      assert.ok(arguments_.some(value => /Google Chrome.app|[\\/]google[\\/]chrome[\\/]chrome|[\\/]google-chrome(?:-stable)?$|Google[\\/]Chrome[\\/]Application[\\/]chrome.exe/iu.test(value)), 'Use Google Chrome, not a substituted browser.');
      if (process.platform === 'darwin') assert.ok(arguments_.includes('--use-mock-keychain'), 'Test Chrome must use the mock keychain.');
      evidence.chrome.push((await browser.call('Browser.getVersion')).product);
      await browser.call('Target.setDiscoverTargets', { discover: true });
      const { targetInfos } = await browser.call('Target.getTargets');
      const page = targetInfos.find(value => value.type === 'page' && value.url.startsWith(`chrome-extension://${extension}/`));
      assert.ok(page, 'Open Kwel Settings in the disposable test Chrome.');
      const session = (await browser.call('Target.attachToTarget', { targetId: page.targetId, flatten: true })).sessionId;
      browser.session = session;
      const state = await browser.evaluate(`chrome.storage.local.get(['kwel.local.profileId','kwel.local.status','kwel.local.enabled','kwel.local.consentVersion'])`, session);
      assert.ok(state['kwel.local.enabled'] === true && state['kwel.local.consentVersion'] === 1 && state['kwel.local.status'] === 'connected', 'Enable Local Connection through Kwel Settings and wait for Connected.');
      browser.profile = state['kwel.local.profileId'];
      assert.ok(typeof browser.profile === 'string' && /^[a-f0-9-]{36}$/iu.test(browser.profile), 'Kwel has no valid test profile ID.');
      const probe = await command(browser.profile, ['profile-probe']);
      assert.ok(probe.profileId === browser.profile, 'Installed Host did not answer the Chrome profile probe.');
      browser.expected = [`${base}/${role}/one`, `${base}/${role}/two`, `${base}/shared`];
    }
    assert.ok(browsers[0].profile !== browsers[1].profile, 'Duplicate profile IDs are not an isolation test.');
    evidence.checks.chromeLaunchedHostAndHelloAck = true;
    for (const [index, browser] of browsers.entries()) {
      const role = ['A', 'B'][index];
      stage(`visit fixture pages ${role}`);
      for (const url of browser.expected) {
        const target = await browser.call('Target.createTarget', { url: 'about:blank' });
        created.push([browser, target.targetId]);
        const session = (await browser.call('Target.attachToTarget', { targetId: target.targetId, flatten: true })).sessionId;
        await browser.call('Page.enable', {}, session);
        const loaded = browser.event(value => value.method === 'Page.loadEventFired' && value.sessionId === session);
        await browser.call('Page.navigate', { url }, session); await loaded;
      }
      stage(`wait for Kwel indexing ${role}`);
      const indexed = await browser.evaluate(`new Promise((resolve,reject)=>{
        const expected=${JSON.stringify(browser.expected)},query=${JSON.stringify(query)};
        let probing=false,dirty=false,done=false,last={count:0,matched:0,error:false};
        const timer=setTimeout(()=>{done=true;chrome.runtime.onMessage.removeListener(changed);resolve({ready:false,...last});},45000);
        async function check(){if(done)return;dirty=true;if(probing)return;probing=true;try{do{
          dirty=false;
          const state=await chrome.runtime.sendMessage({apiVersion:1,requestId:crypto.randomUUID(),method:'GET_ARCHIVE_STATUS',payload:{}});
          if(!state.ok)throw new Error('Kwel status unavailable');
          const archive=state.data;
          const response=await chrome.runtime.sendMessage({apiVersion:1,requestId:crypto.randomUUID(),method:'SEARCH_PAGES',payload:{query}});
          const jobs=await chrome.runtime.sendMessage({apiVersion:1,requestId:crypto.randomUUID(),method:'GET_JOB_STATUS',payload:{}});
          const after=await chrome.runtime.sendMessage({apiVersion:1,requestId:crypto.randomUUID(),method:'GET_ARCHIVE_STATUS',payload:{}});
          if(!jobs.ok || !after.ok)throw new Error('Kwel indexing state unavailable');
          const revision=a=>[a.generation,a.inputVersion,a.searchVersion,a.publishedResultRevision];
          const stable=JSON.stringify(revision(archive))===JSON.stringify(revision(after.data));
          last={count:response.ok?response.data.pages.length:0,matched:response.ok?expected.filter(url=>response.data.pages.some(page=>page.url===url)).length:0,error:!response.ok};
          if(response.ok && !response.data.partial && last.matched===expected.length && stable && !jobs.data.some(job=>job.status==='pending'||job.status==='running') && archive.analysisState==='ready' && archive.modelInputVersion===archive.inputVersion){done=true;clearTimeout(timer);chrome.runtime.onMessage.removeListener(changed);resolve({ready:true,pages:response.data.pages,revision:revision(archive)});}
        }while(dirty&&!done);}catch{done=true;clearTimeout(timer);chrome.runtime.onMessage.removeListener(changed);reject(new Error('Kwel fixture search failed'));}finally{probing=false;}}
        function changed(value){if(value.target==='kwel-ui' && value.kind==='state-changed')void check();}
        chrome.runtime.onMessage.addListener(changed);void check();
      })`, browser.session);
      assert.ok(indexed.ready, `Kwel fixture indexing failed (results=${indexed.count}, matched=${indexed.matched}, API error=${indexed.error}).`);
      const pages = indexed.pages;
      assert.ok(pages.length === browser.expected.length && pages.every(value => browser.expected.includes(value.url)), 'Chrome profile histories are mixed or the fixture is incomplete.');
      stage(`compare CLI ranking ${role}`);
      const result = await command(browser.profile, ['search', '--query', query, '--limit', '20']);
      const after = await browser.evaluate(`chrome.runtime.sendMessage({apiVersion:1,requestId:crypto.randomUUID(),method:'GET_ARCHIVE_STATUS',payload:{}}).then(value=>{if(!value.ok)throw new Error('Kwel status unavailable');const a=value.data;return [a.generation,a.inputVersion,a.searchVersion,a.publishedResultRevision];})`, browser.session);
      assert.deepEqual(after, indexed.revision, 'Kwel model changed during the ranking comparison; this is not transport evidence.');
      const sameUrls = JSON.stringify(result.items.map(item => item.url)) === JSON.stringify(pages.map(item => item.url));
      const sameTitles = JSON.stringify(result.items.map(item => item.title)) === JSON.stringify(pages.map(item => item.title));
      const sameDomains = JSON.stringify(result.items.map(item => item.domain)) === JSON.stringify(pages.map(item => item.domain));
      if (!sameUrls || !sameTitles || !sameDomains) console.error(`Fixture comparison: same ordered URLs=${sameUrls}, same titles=${sameTitles}, same domains=${sameDomains}.`);
      assert.ok(JSON.stringify(result.items.map(({ title, url, domain }) => ({ title, url, domain }))) === JSON.stringify(pages.map(({ title, url, domain }) => ({ title, url, domain }))), 'CLI changed Kwel ranking or result data.');
      browser.results = result.items;
      stage(`empty query ${role}`);
      await command(browser.profile, ['search', '--query', '', '--limit', '20']);
    }
    evidence.checks.rankingUrlsEmptySearchAndProfileIsolation = true;
    const [a, b] = browsers, selected = a.results.find(value => value.url === `${base}/shared`);
    assert.ok(selected, 'The shared two-profile fixture is missing.');
    Object.assign(evidence.checks, await verifyPageOpening({ a, b, selected, query, command, created }));
    writeFileSync(output, JSON.stringify(evidence, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
    console.log('Real Google Chrome / Kwel / installed Host E2E passed. Alfred UI and OS approval prompts require separate manual evidence.');
  } finally {
    for (const [browser, targetId] of created) await browser.call('Target.closeTarget', { targetId }).catch(() => {});
    for (const browser of browsers) browser.close();
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  main().catch(error => { console.error(`Chrome E2E failed at ${phase}: ${error instanceof assert.AssertionError ? error.message : 'private Chrome responses were not logged.'}`); process.exitCode = 1; });
}
