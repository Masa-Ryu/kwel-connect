// Guard tests only; these never attest a real Chrome/Native/Alfred run.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { readFileSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { verifyPageOpening } from '../test-chrome-e2e.mjs';
const script = fileURLToPath(new URL('../test-chrome-e2e.mjs', import.meta.url));

test('the public unpacked test key preserves only the published Host identity', () => {
  const value=JSON.parse(readFileSync(new URL('../../config/chrome-e2e-extension.json', import.meta.url),'utf8'));
  const digest=createHash('sha256').update(Buffer.from(value.publicKey,'base64')).digest().subarray(0,16);
  const id=[...digest].flatMap(byte=>[byte>>4,byte&15]).map(value=>String.fromCharCode(97+value)).join('');
  assert.equal(id,value.extensionId);
  assert.equal(id,JSON.parse(readFileSync(new URL('../../config/distribution.json',import.meta.url),'utf8')).chromeExtensionId);
});

test('Chrome plans allocate fresh, marked temporary profiles and reject unsafe endpoints', () => {
  const prepared = JSON.parse(execFileSync(process.execPath, [script, 'prepare'], { encoding: 'utf8', timeout: 5000 }));
  const root = path.dirname(prepared.plan);
  try {
    const relative = path.relative(realpathSync(tmpdir()), realpathSync(root));
    assert.ok(!relative.startsWith('..') && !path.isAbsolute(relative));
    const plan = JSON.parse(readFileSync(prepared.plan, 'utf8'));
    assert.notEqual(plan.profiles.A, plan.profiles.B);
    for (const role of ['A', 'B']) {
      assert.deepEqual(JSON.parse(readFileSync(path.join(plan.profiles[role], '.kwel-connect-e2e.json'), 'utf8')), { nonce: plan.nonce, role });
    }
    for (const [a, b, expected] of [
      ['https://remote.invalid/', 'http://127.0.0.1:9223', 'CDP must be a loopback test endpoint'],
      ['http://127.0.0.1:9222', 'http://127.0.0.1:9222', 'Two separate Chrome test profiles'],
    ]) {
      const result = spawnSync(process.execPath, [script, '--plan', prepared.plan, '--cli', process.execPath, '--a', a, '--b', b, '--out', path.join(root, 'unused.json')], { encoding: 'utf8', timeout: 5000 });
      assert.equal(result.status, 1);
      assert.match(result.stderr, new RegExp(expected));
    }
  } finally { rmSync(root, { recursive: true }); }
});

for (const [fault, expected] of [
  ['duplicate', /created, closed or navigated/],
  ['reload', /reloaded or lost its document state/],
  ['inactive', /were not activated/],
  ['other-profile', /changed profile B targets/],
]) {
  test(`the opening verifier rejects a false success with ${fault} behavior (guard only)`, async () => {
    const url = 'http://127.0.0.1/fixture';
    let opened = false;
    const browser = role => ({
      profile: role, session: role,
      targets: [{ type: 'page', targetId: role, url }],
      async call(method) {
        if (method === 'Target.getTargets') return { targetInfos: this.targets };
        if (method === 'Target.attachToTarget') return { sessionId: 'fixture' };
        throw new Error('Unexpected guard command.');
      },
      async evaluate(expression) {
        if (expression.includes('query({active:true})')) return [role === 'A' ? 1 : 2];
        if (expression.includes('return {id:tab.id,windowId:tab.windowId}')) return { id: 1, windowId: 1 };
        if (expression.startsWith('globalThis.__kwelOpenState===')) return !(opened && fault === 'reload');
        if (expression.includes('return tab.active')) return fault !== 'inactive';
        return true;
      },
    });
    const a = browser('A'), b = browser('B');
    const command = async (profile, _args, failureCode) => {
      if (profile === 'A' && !failureCode) {
        opened = true;
        if (fault === 'duplicate') a.targets.push({ type: 'page', targetId: 'duplicate', url });
        if (fault === 'other-profile') b.targets.push({ type: 'page', targetId: 'foreign', url });
      }
      return { ok: true };
    };
    await assert.rejects(verifyPageOpening({ a, b, selected: { url, openRef: 'opaque' }, query: 'fixture', command, created: [] }), expected);
  });
}
