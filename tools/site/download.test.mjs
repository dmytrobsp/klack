// Unit tests for the download button (site/js/download.js). Run: node --test "tools/site/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { detectOS, choose, enhance } from '../../site/js/download.js';

const rel = JSON.parse(readFileSync(new URL('../../site/data/release.json', import.meta.url)));
const UA = {
  linux: 'Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0',
  win: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36',
  mac: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
  ipad: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15',
  android: 'Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Mobile Safari/537.36',
};

test('detectOS', () => {
  assert.equal(detectOS({ userAgent: UA.linux, platform: 'Linux x86_64', maxTouchPoints: 0 }), 'linux');
  assert.equal(detectOS({ userAgent: UA.win, platform: 'Win32', maxTouchPoints: 0 }), 'windows');
  assert.equal(detectOS({ userAgent: UA.mac, platform: 'MacIntel', maxTouchPoints: 0 }), 'macos');
  assert.equal(detectOS({ userAgent: UA.ipad, platform: 'MacIntel', maxTouchPoints: 5 }), 'mobile');
  assert.equal(detectOS({ userAgent: UA.android, platform: 'Linux armv81', maxTouchPoints: 5 }), 'mobile');
  assert.equal(detectOS({ userAgent: '', platform: '', maxTouchPoints: 0 }), 'unknown');
  assert.equal(detectOS({}), 'unknown');
});

test('choose: macOS offers both zips, labelled', () => {
  const c = choose('macos', rel);
  assert.deepEqual(c.assets.map(a => a.label), ['Apple Silicon', 'Intel']);
  assert.deepEqual(c.assets.map(a => a.platform), ['aarch64-macos', 'x86_64-macos']);
});

test('choose: windows and linux', () => {
  assert.equal(choose('windows', rel).label, 'DOWNLOAD FOR WINDOWS');
  assert.equal(choose('windows', rel).assets[0].platform, 'x86_64-windows');
  assert.equal(choose('linux', rel).label, 'DOWNLOAD FOR LINUX');
  assert.equal(choose('linux', rel).assets[0].platform, 'x86_64-linux');
});

test('choose: mobile and unknown go to all downloads', () => {
  for (const os of ['mobile', 'unknown']) {
    const c = choose(os, rel);
    assert.equal(c.label, 'ALL DOWNLOADS');
    assert.equal(c.href, '#get');
    assert.deepEqual(c.assets, []);
  }
});

test('choose: an unknown platform is never chosen but stays in the release', () => {
  const r = { ...rel, assets: [...rel.assets, { name: 'x-riscv64-linux.zip', platform: 'riscv64-linux', url: 'https://example.invalid/x', size: 1 }] };
  for (const os of ['linux', 'macos', 'windows', 'mobile', 'unknown'])
    assert.ok(!choose(os, r).assets.some(a => a.platform === 'riscv64-linux'));
  assert.ok(r.assets.some(a => a.platform === 'riscv64-linux'));
});

function fakeDoc() {
  const area = { innerHTML: 'static' };
  return { area, querySelector: s => (s === '.dl-area' ? area : null) };
}

test('enhance rewrites the region for the visitor', () => {
  let d = fakeDoc();
  enhance(d, rel, { userAgent: UA.win, platform: 'Win32', maxTouchPoints: 0 });
  assert.match(d.area.innerHTML, /class="dl" data-os="windows" href="[^"]*x86_64-windows\.zip">DOWNLOAD FOR WINDOWS</);
  assert.ok(d.area.innerHTML.includes(`v${rel.version}`));
  const win = rel.assets.find(a => a.platform === 'x86_64-windows');
  assert.ok(d.area.innerHTML.includes(`${(win.size / 1e6).toFixed(1)} MB`));

  d = fakeDoc();
  enhance(d, rel, { userAgent: UA.mac, platform: 'MacIntel', maxTouchPoints: 0 });
  assert.match(d.area.innerHTML, />Apple Silicon</);
  assert.match(d.area.innerHTML, />Intel</);
  assert.match(d.area.innerHTML, /aarch64-macos\.zip/);

  d = fakeDoc();
  enhance(d, rel, { userAgent: UA.android, platform: 'Linux armv81', maxTouchPoints: 5 });
  assert.match(d.area.innerHTML, /data-os="mobile" href="#get">ALL DOWNLOADS</);
});

test('enhance leaves the static region alone with no region or no release', () => {
  const d = fakeDoc();
  enhance(d, null, { userAgent: UA.win });
  assert.equal(d.area.innerHTML, 'static');
  assert.doesNotThrow(() => enhance({ querySelector: () => null }, rel, { userAgent: UA.win }));
});

test('enhance shows no version rather than "vundefined"', () => {
  for (const os of [UA.win, UA.android]) {
    const d = fakeDoc();
    const { version, ...noVer } = rel;
    enhance(d, noVer, { userAgent: os, platform: '', maxTouchPoints: 0 });
    assert.doesNotMatch(d.area.innerHTML, /undefined/);
    assert.doesNotMatch(d.area.innerHTML, />v\d/);
  }
});
