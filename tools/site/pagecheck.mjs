// The landing page's check on phones and desktops: drives headless Chrome over the DevTools protocol
// (like tools/cdp.mjs) and prints ONE JSON object. Node 24, no packages.
//
//   node tools/site/pagecheck.mjs URL [--reduced-motion | --force-prefers-reduced-motion]
//        [--shots DIR] [--full] [--hero-at SECONDS] [--user-agent UA] [--attach PORT] [--scratch DIR] [--port N]
//
// Per width (360, 390, 768, 1280): scrollWidth <= innerWidth (no sideways scroll), the first screen's
// stage scale, console errors, requests to other hosts (there must be none), the transfer size.
// Once, at 1280: the headline loop moves (or, under reduced motion, hangs still), it pauses while the
// headline is off screen, and a capture opens enlarged and readable, and closes with Esc.
// --shots saves a screenshot per width (the viewport; --full: the whole page). --hero-at freezes the
// loop's clock at that stage time first (for looking at one frame); --user-agent poses as another system
// (the download button picks by it). Without --attach it launches its own
// Chrome (flatpak com.google.Chrome, or $CHROME) with a fresh profile, closes it and deletes the profile.
// Exit code 0 only when every check passes.
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const WIDTHS = [360, 390, 768, 1280];
const HEIGHT = { 360: 780, 390: 844, 768: 1024, 1280: 900 };

const argv = process.argv.slice(2);
const opt = { url: null, reduced: false, shots: null, full: false, heroAt: null, attach: null, scratch: null, port: 9333, ua: null };
for (let i = 0; i < argv.length; i++) {
  const a = argv[i];
  if (a === '--reduced-motion' || a === '--force-prefers-reduced-motion') opt.reduced = true;
  else if (a === '--shots') opt.shots = argv[++i];
  else if (a === '--full') opt.full = true;
  else if (a === '--hero-at') opt.heroAt = Number(argv[++i]);
  else if (a === '--attach') opt.attach = Number(argv[++i]);
  else if (a === '--scratch') opt.scratch = argv[++i];
  else if (a === '--port') opt.port = Number(argv[++i]);
  else if (a === '--user-agent') opt.ua = argv[++i];
  else if (!a.startsWith('--') && !opt.url) opt.url = a;
  else { console.error('unknown argument: ' + a); process.exit(2); }
}
if (!opt.url) { console.error('usage: node tools/site/pagecheck.mjs URL [--reduced-motion] [--shots DIR] [--full] [--hero-at S] [--attach PORT]'); process.exit(2); }

const sleep = ms => new Promise(r => setTimeout(r, ms));

// ---- the browser ----
let child = null, profile = null, port = opt.attach;
if (!port) {
  const base = realpathSync(opt.scratch || tmpdir());
  profile = mkdtempSync(join(base, 'pagecheck-'));
  port = opt.port;
  const flags = ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}/prof`, '--no-first-run',
    '--no-default-browser-check', '--hide-scrollbars', '--mute-audio', '--disable-extensions', 'about:blank'];
  child = process.env.CHROME
    ? spawn(process.env.CHROME, flags, { stdio: 'ignore' })
    : spawn('flatpak', ['run', `--filesystem=${profile}`, 'com.google.Chrome', ...flags], { stdio: 'ignore' });
}
async function json(path) { return (await fetch(`http://127.0.0.1:${port}${path}`)).json(); }
let version = null;
for (let i = 0; i < 100 && !version; i++) { try { version = await json('/json/version'); } catch { await sleep(200); } }
if (!version) { await cleanup(); console.error('no browser on port ' + port); process.exit(2); }

function connect(wsUrl) {
  const ws = new WebSocket(wsUrl);
  let id = 0; const pending = new Map(), handlers = [];
  ws.onmessage = m => {
    const msg = JSON.parse(m.data);
    if (msg.id && pending.has(msg.id)) {
      const { ok, fail } = pending.get(msg.id); pending.delete(msg.id);
      msg.error ? fail(new Error(msg.error.message)) : ok(msg.result);
    } else if (msg.method) for (const h of handlers) h(msg.method, msg.params);
  };
  const send = (method, params = {}) => new Promise((ok, fail) => {
    const n = ++id; pending.set(n, { ok, fail }); ws.send(JSON.stringify({ id: n, method, params }));
  });
  return { ws, send, on: h => handlers.push(h), open: new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = fail; }) };
}

const browser = connect(version.webSocketDebuggerUrl);
await browser.open;
const { targetId } = await browser.send('Target.createTarget', { url: 'about:blank' });
const targets = await json('/json');
const page = connect(targets.find(t => t.id === targetId).webSocketDebuggerUrl);
await page.open;
const { send } = page;

const origin = new URL(opt.url).host;
let log = null;   // what one load collects
page.on((method, p) => {
  if (!log) return;
  if (method === 'Network.requestWillBeSent') {
    const u = p.request.url;
    if (/^https?:/.test(u) && new URL(u).host !== origin) log.foreign.push(u);
  } else if (method === 'Network.loadingFinished') log.bytes += p.encodedDataLength;
  else if (method === 'Network.loadingFailed' && !p.canceled) log.errors.push('load failed: ' + p.errorText);
  else if (method === 'Runtime.consoleAPICalled' && (p.type === 'error' || p.type === 'assert'))
    log.errors.push('console: ' + p.args.map(a => a.value ?? a.description ?? '').join(' '));
  else if (method === 'Runtime.exceptionThrown')
    log.errors.push('exception: ' + (p.exceptionDetails.exception?.description || p.exceptionDetails.text));
  else if (method === 'Log.entryAdded' && p.entry.level === 'error') log.errors.push('log: ' + p.entry.text + (p.entry.url ? ' ' + p.entry.url : ''));
  else if (method === 'Page.loadEventFired') log.loaded = true;
});
await send('Page.enable'); await send('Runtime.enable'); await send('Log.enable'); await send('Network.enable');
await send('Network.setCacheDisabled', { cacheDisabled: true });
if (opt.reduced) await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-reduced-motion', value: 'reduce' }] });
if (opt.ua) await send('Emulation.setUserAgentOverride', { userAgent: opt.ua, platform: /Mac/.test(opt.ua) ? 'MacIntel' : '' });
if (opt.heroAt !== null) await send('Page.addScriptToEvaluateOnNewDocument', { source: 'window.__clock = 0; performance.now = () => window.__clock;' });

const evaluate = async expr => {
  const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
};
async function shot(file) {
  const r = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: opt.full });
  writeFileSync(file, Buffer.from(r.data, 'base64'));
}

async function load(width) {
  const height = HEIGHT[width];
  await send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: width < 700 });
  await send('Emulation.setTouchEmulationEnabled', width < 700 ? { enabled: true, maxTouchPoints: 5 } : { enabled: false });
  log = { foreign: [], bytes: 0, errors: [], loaded: false };
  await send('Page.navigate', { url: opt.url });
  for (let i = 0; i < 100 && !log.loaded; i++) await sleep(100);
  await evaluate('document.fonts.ready.then(() => true)');
  await sleep(600);   // the download button's release.json, layout settling
  const m = await evaluate(`(() => {
    const st = document.querySelector('.stage'), r = st ? st.getBoundingClientRect() : null;
    return { innerWidth, scrollWidth: document.documentElement.scrollWidth,
      stageScale: r ? Math.round(r.width / st.offsetWidth * 1000) / 1000 : null,
      stageBox: r ? [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)] : null };
  })()`);
  // on a phone the layout viewport widens to whatever overflows, so compare with the device width too
  return { width, ...m, fits: m.scrollWidth <= m.innerWidth && m.innerWidth === width };
}

// the zoom: Enter on the first feature capture opens it readable (scrolling sideways on a phone); Esc closes it
async function zoomCheck(width) {
  const key = (key, code, vk, text) => Promise.all(['keyDown', 'keyUp'].map(type => send('Input.dispatchKeyEvent',
    { type, key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, ...(type === 'keyDown' && text ? { text } : {}) })));
  const z = await evaluate(`(() => {
    const f = document.querySelector('#features .capture'); if (!f) return null;
    f.scrollIntoView({ block: 'center', behavior: 'instant' }); f.focus();
    const g = f.querySelector('.grid');
    return { focusable: document.activeElement === f, hint: f.querySelector('.zoom-hint')?.textContent || null,
      slotFontPx: Math.round(parseFloat(getComputedStyle(g).fontSize) * 10) / 10 };
  })()`);
  if (!z) return null;
  await key('Enter', 'Enter', 13, '\r'); await sleep(300);
  Object.assign(z, await evaluate(`(() => {
    const d = document.querySelector('dialog.zoom[open]'); if (!d) return { opens: false };
    const g = d.querySelector('.grid');
    return { opens: true, zoomFontPx: Math.round(parseFloat(getComputedStyle(g).fontSize) * 10) / 10,
      scrollsSideways: d.scrollWidth > d.clientWidth, pageFits: document.documentElement.scrollWidth <= innerWidth };
  })()`));
  if (opt.shots && z.opens) await shot(join(opt.shots, `zoom${width}.png`));
  await key('Escape', 'Escape', 27); await sleep(300);
  Object.assign(z, await evaluate(`({ closesOnEsc: !document.querySelector('dialog.zoom[open]'), focusBack: document.activeElement === document.querySelector('#features .capture') })`));
  return z;
}

const clothD = () => evaluate(`document.getElementById('hero-cloth')?.getAttribute('d') || ''`);

const out = { url: opt.url, reducedMotion: opt.reduced, widths: [], consoleErrors: [], foreignRequests: [], transferBytes: {} };
try {
  for (const w of WIDTHS) {
    const r = await load(w);
    if (opt.heroAt !== null) { await evaluate(`window.__clock = ${opt.heroAt * 1000}`); await sleep(400); }
    if (opt.shots) { mkdirSync(opt.shots, { recursive: true }); await shot(join(opt.shots, `w${w}.png`)); }
    out.widths.push({ width: r.width, innerWidth: r.innerWidth, scrollWidth: r.scrollWidth, fits: r.fits, stageScale: r.stageScale, stageBox: r.stageBox, errors: log.errors.length });
    out.transferBytes[w] = log.bytes;
    out.consoleErrors.push(...log.errors.map(e => `${w}: ${e}`));
    out.foreignRequests.push(...log.foreign);
  }
  // the behaviour checks, at the last width (1280), on a fresh load
  if (opt.heroAt === null) {
    await load(1280);
    const a = await clothD(); await sleep(opt.reduced ? 2500 : 2200); const b = await clothD();
    const hand = await evaluate(`document.getElementById('hero-hand').style.opacity`);
    out.hero = opt.reduced ? { moves: a !== b, handOpacity: hand, hangs: a === b && a !== '' }
      : { moves: a !== b };
    if (!opt.reduced) {
      await evaluate(`document.getElementById('numbers').scrollIntoView({ behavior: 'instant' }); true`); await sleep(500);
      const c = await clothD(); await sleep(1500); const d = await clothD();
      out.hero.pausedOffScreen = c === d;
      await evaluate(`scrollTo({ top: 0, behavior: 'instant' }); true`); await sleep(600);
      const e = await clothD(); await sleep(1200); const f = await clothD();
      out.hero.resumesOnScreen = e !== f;
    }
    out.zoom = { 1280: await zoomCheck(1280) };
    out.consoleErrors.push(...log.errors.map(e => `checks: ${e}`));
    out.foreignRequests.push(...log.foreign);
    await load(390);
    out.zoom[390] = await zoomCheck(390);
    out.consoleErrors.push(...log.errors.map(e => `checks: ${e}`));
    out.foreignRequests.push(...log.foreign);
  }
} catch (e) {
  out.fatal = String(e && e.message || e);
} finally {
  log = null;
}
out.foreignRequests = [...new Set(out.foreignRequests)];
out.transferMB = Math.round(Math.max(...Object.values(out.transferBytes)) / 1e5) / 10;
const heroOk = !out.hero || (opt.reduced ? out.hero.hangs && out.hero.handOpacity === '0'
  : out.hero.moves && out.hero.pausedOffScreen && out.hero.resumesOnScreen);
const zoomOk = !out.zoom || Object.values(out.zoom).every(z => z && z.focusable && z.hint && z.opens && z.zoomFontPx >= 11 && z.pageFits && z.closesOnEsc && z.focusBack);
out.ok = !out.fatal && out.widths.length === WIDTHS.length && out.widths.every(w => w.fits) && !out.consoleErrors.length && !out.foreignRequests.length && heroOk && zoomOk;
console.log(JSON.stringify(out, null, 1));

await send('Target.closeTarget', { targetId }).catch(() => {});
page.ws.close();
if (child) { await browser.send('Browser.close').catch(() => {}); }
browser.ws.close();
await cleanup();
process.exit(out.ok ? 0 : 1);

async function cleanup() {
  if (child) {
    for (let i = 0; i < 50 && child.exitCode === null && child.signalCode === null; i++) await sleep(100);
    if (child.exitCode === null && child.signalCode === null) child.kill();
    await sleep(300);
  }
  if (profile) rmSync(profile, { recursive: true, force: true });
}
