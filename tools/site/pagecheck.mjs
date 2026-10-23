// The landing page's check on phones and desktops: drives headless Chrome over the DevTools protocol
// (like tools/cdp.mjs) and prints ONE JSON object. Node 24, no packages.
//
//   node tools/site/pagecheck.mjs URL [--reduced-motion | --force-prefers-reduced-motion]
//        [--shots DIR] [--full] [--hero-at SECONDS] [--user-agent UA] [--attach PORT] [--scratch DIR] [--port N]
//
// Per width (320, 360, 390, 768, 1280): scrollWidth <= innerWidth (no sideways scroll), the first screen's
// stage scale, the first screen's download button (rendered ≥ 40 px tall everywhere; below 760 px out of the
// scaled stage, its meta line ≥ 14 px), the hero's one-liner when the release has one (≥ 40 px tall, inside the
// viewport, the whole call to action above the next section), console errors, requests to other hosts (there must be none), the transfer size,
// the copy buttons (one in each marked command, at least 24 px, inside the viewport, never over the command's text,
// never making it take more lines; the hero's one-liner on one line from 760 px).
// Once, at 1280: the headline loop moves (or, under reduced motion, hangs still), it pauses while the
// headline is off screen, and a capture opens enlarged and readable (at 390 too), the page behind doesn't scroll,
// and it closes with Esc. At 1280, 390 and 320 a copy button copies its command (a click; on a phone a tap beside the
// button, in its hit area; Tab to one shows the focus ring and Enter copies), its feedback goes back after a while, and
// without the clipboard API the command is selected. The page's clipboard and execCommand are stubbed on every load:
// the real clipboard is never touched.
// --shots saves a screenshot per width (the viewport; --full: the whole page). --hero-at freezes the
// loop's clock at that stage time first (for looking at one frame); --user-agent poses as another system
// (the download button picks by it). Without --attach it launches its own
// Chrome (flatpak com.google.Chrome, or $CHROME) with a fresh profile and always — error or not — closes it
// (Browser.close, else its process group and any process on that profile) and deletes the profile.
// Exit code 0 only when every check passes.
import { spawn } from 'node:child_process';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, realpathSync, readdirSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const WIDTHS = [320, 360, 390, 768, 1280];
const HEIGHT = { 320: 568, 360: 780, 390: 844, 768: 1024, 1280: 900 };

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
let child = null, profile = null, port = opt.attach, browser = null, page = null, targetId = null, send = null;
let log = null;   // what one load collects
const origin = new URL(opt.url).host;
const out = { url: opt.url, reducedMotion: opt.reduced, widths: [], consoleErrors: [], foreignRequests: [], transferBytes: {} };

async function json(path) { return (await fetch(`http://127.0.0.1:${port}${path}`, { signal: AbortSignal.timeout(2000) })).json(); }
async function alive() { try { await json('/json/version'); return true; } catch { return false; } }
const within = (p, ms) => Promise.race([p, sleep(ms)]);

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
  const call = (method, params = {}) => new Promise((ok, fail) => {
    const n = ++id; pending.set(n, { ok, fail }); ws.send(JSON.stringify({ id: n, method, params }));
  });
  return { ws, send: call, on: h => handlers.push(h), open: new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = fail; }) };
}

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
    ${COPY_TEXT}
    const st = document.querySelector('.stage'), r = st ? st.getBoundingClientRect() : null;
    const dl = document.querySelector('.dl-area .dl'), b = dl ? dl.getBoundingClientRect() : null;
    const meta = document.querySelector('.dl-area .dl-meta');
    const cmd = document.querySelector('.dl-area .dl-cmd'), c = cmd ? cmd.getBoundingClientRect() : null;
    const area = document.querySelector('.dl-area'), feat = document.getElementById('features');
    const scaleOf = e => e.getBoundingClientRect().width / (e.offsetWidth || 1);   // rendered / layout size
    return { innerWidth, scrollWidth: document.documentElement.scrollWidth,
      stageScale: r ? Math.round(r.width / st.offsetWidth * 1000) / 1000 : null,
      stageBox: r ? [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)] : null,
      button: b ? { label: dl.textContent, box: [Math.round(b.left), Math.round(b.top), Math.round(b.width), Math.round(b.height)],
        inStage: !!dl.closest('.stage') } : null,
      metaPx: meta ? Math.round(parseFloat(getComputedStyle(meta).fontSize) * scaleOf(meta) * 10) / 10 : null,
      oneLiner: c ? { text: cmd.textContent, box: [Math.round(c.left), Math.round(c.top), Math.round(c.width), Math.round(c.height)],
        inView: c.left >= 0 && c.right <= innerWidth } : null,
      aboveNext: !area || !feat || area.getBoundingClientRect().bottom <= feat.getBoundingClientRect().top + 1,
      copy: (() => {
        const marked = [...document.querySelectorAll('[data-copy]')], bad = [];
        const overlap = (a, b) => a.left < b.right - 0.5 && b.left < a.right - 0.5 && a.top < b.bottom - 0.5 && b.top < a.bottom - 0.5;
        let buttons = 0, heroLines = null;
        for (const el of marked) {
          const b = el.querySelector(':scope > button.copy'), cmd = copyText(el);
          if (!b) { bad.push('no button: ' + cmd); continue; }
          buttons++;
          const r = b.getBoundingClientRect();
          if (r.width < 23.5 || r.height < 23.5) bad.push('small: ' + cmd);
          if (r.left < 0 || r.right > innerWidth) bad.push('outside: ' + cmd);
          // the lines the command's text takes (no word under the button), with the button and without it (taken out, as
          // with JS off, then put back); words only: a space hanging at a line's end (pre-wrap) is no text to cover
          const lines = shown => {
            const tops = new Set();
            for (const n of el.childNodes) {
              if (n.nodeType !== 3) continue;
              for (const w of n.data.matchAll(/\\S+/g)) {
                const range = document.createRange(); range.setStart(n, w.index); range.setEnd(n, w.index + w[0].length);
                for (const t of range.getClientRects()) { if (shown && overlap(t, r)) bad.push('over the text: ' + cmd); tops.add(Math.round(t.top)); }
              }
            }
            return tops.size;
          };
          const withIt = lines(true), at = b.nextSibling;
          b.remove(); const without = lines(false); el.insertBefore(b, at);
          if (withIt !== without) bad.push('wraps more for its button (' + without + ' → ' + withIt + ' lines): ' + cmd);
          if (el.matches('.dl-area .dl-cmd')) heroLines = withIt;
        }
        return { marked: marked.length, buttons, heroLines, bad: [...new Set(bad)] };
      })() };
  })()`);
  // on a phone the layout viewport widens to whatever overflows, so compare with the device width too
  return { width, ...m, fits: m.scrollWidth <= m.innerWidth && m.innerWidth === width };
}

// the zoom: Enter on the first feature capture opens it readable (scrolling sideways on a phone), the page behind
// doesn't scroll under the wheel; Esc closes it
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
  if (z.opens) {
    const y0 = await evaluate('scrollY');
    await send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: 4, y: HEIGHT[width] - 4, deltaX: 0, deltaY: 600 });
    await sleep(400);
    z.pageScrollLocked = (await evaluate('scrollY')) === y0;
  }
  if (opt.shots && z.opens) await shot(join(opt.shots, `zoom${width}.png`));
  await key('Escape', 'Escape', 27); await sleep(300);
  Object.assign(z, await evaluate(`({ closesOnEsc: !document.querySelector('dialog.zoom[open]'), focusBack: document.activeElement === document.querySelector('#features .capture') })`));
  return z;
}

// what a copy button should copy: the marked element's own text, without the prompt (.ps) or the button
const COPY_TEXT = `const copyText = el => [...el.childNodes].filter(n => n.nodeType === 3).map(n => n.data).join('').trim();`;
// on every load, before the page's scripts: the clipboard API and execCommand only record what they were given
const CLIPBOARD_STUB = `window.__copied = []; window.__oldWay = [];
  if (navigator.clipboard) {
    navigator.clipboard.writeText = t => { window.__copied.push(t); return Promise.resolve(); };
    navigator.clipboard.write = () => Promise.reject(new Error('stubbed'));
  }
  document.execCommand = c => { window.__oldWay.push([c, String(getSelection())]); return false; };`;

// the copy buttons: the hero's copies by a click (on a phone by a tap in its hit area, beside the button), its feedback
// goes back; Tab onto the first one in Get it shows the focus ring and Enter copies; without the clipboard API a
// package's command is selected (execCommand stubbed: it says it could not copy)
async function copyCheck(width) {
  const phone = width < 700;
  const key = (key, code, vk, text) => Promise.all(['keyDown', 'keyUp'].map(type => send('Input.dispatchKeyEvent',
    { type, key, code, windowsVirtualKeyCode: vk, nativeVirtualKeyCode: vk, ...(type === 'keyDown' && text ? { text } : {}) })));
  // the point to press: the button's middle; on a phone 8 px above it, inside its hit area but outside the button
  const aim = sel => evaluate(`(() => {
    ${COPY_TEXT}
    const b = document.querySelector(${JSON.stringify(sel)}); if (!b) return null;
    b.scrollIntoView({ block: 'center', behavior: 'instant' });
    const r = b.getBoundingClientRect(), x = r.left + r.width / 2, y = ${phone} ? r.top - 8 : r.top + r.height / 2;
    return { x, y, cmd: copyText(b.parentElement), hits: document.elementFromPoint(x, y)?.closest('.copy') === b,
      coarse: matchMedia('(pointer: coarse)').matches };
  })()`);
  const press = async ({ x, y }) => {
    if (phone) for (const type of ['touchStart', 'touchEnd'])
      await send('Input.dispatchTouchEvent', { type, touchPoints: type === 'touchEnd' ? [] : [{ x, y }] });
    else for (const type of ['mousePressed', 'mouseReleased'])
      await send('Input.dispatchMouseEvent', { type, x, y, button: 'left', clickCount: 1 });
    await sleep(250);
  };
  const state = sel => evaluate(`(() => { const b = document.querySelector(${JSON.stringify(sel)});
    return { state: b?.dataset.state ?? null, said: document.querySelector('.copy-status')?.textContent ?? null,
      copied: window.__copied.slice(), oldWay: window.__oldWay.slice(), selection: String(getSelection()) }; })()`);
  const c = {};
  const hero = await aim('.dl-area .dl-cmd > .copy');
  if (hero) {
    await press(hero);
    const s = await state('.dl-area .dl-cmd > .copy');
    await sleep(1800);
    const later = await state('.dl-area .dl-cmd > .copy');
    c.hero = { cmd: hero.cmd, hits: hero.hits, coarse: hero.coarse, copied: s.copied, state: s.state, said: s.said,
      resets: later.state === null && later.said === '' };
    c.hero.ok = hero.hits && s.copied.length === 1 && s.copied[0] === hero.cmd && s.state === 'copied' && s.said === 'Copied'
      && c.hero.resets && (!phone || hero.coarse);
  }
  const first = await evaluate(`(() => {
    const t = document.querySelector('#get .copy'); if (!t) return null;
    const all = [...document.querySelectorAll('a[href], button, [tabindex]')].filter(e => e.tabIndex >= 0);
    const before = all[all.indexOf(t) - 1]; before.focus(); return true; })()`);
  if (first) {
    await key('Tab', 'Tab', 9);
    const f = await evaluate(`(() => { ${COPY_TEXT} const t = document.querySelector('#get .copy'), a = document.activeElement, s = getComputedStyle(a);
      return { focused: a === t, ring: a.matches(':focus-visible') && s.outlineStyle === 'solid' && parseFloat(s.outlineWidth) >= 2,
        cmd: copyText(t.parentElement), before: window.__copied.length }; })()`);
    await key('Enter', 'Enter', 13, '\r');
    await sleep(250);
    const s = await state('#get .copy');
    c.keyboard = { ...f, copied: s.copied.slice(f.before), state: s.state };
    c.keyboard.ok = f.focused && f.ring && c.keyboard.copied.length === 1 && c.keyboard.copied[0] === f.cmd && s.state === 'copied';
  }
  await sleep(1000);   // Tab's focus scroll is smooth (scroll-behavior): let it end before aiming
  const pkg = await evaluate(`(() => { Object.defineProperty(navigator, 'clipboard', { configurable: true, get: () => undefined });
    return !!document.querySelector('.pkgs .copy'); })()`) && await aim('.pkgs .copy');
  if (pkg) {
    const before = (await state('.pkgs .copy')).copied.length;
    await press(pkg);
    const s = await state('.pkgs .copy');
    c.fallback = { cmd: pkg.cmd, hits: pkg.hits, oldWay: s.oldWay, selection: s.selection, state: s.state, said: s.said };
    c.fallback.ok = pkg.hits && s.copied.length === before && s.oldWay.length === 1 && s.oldWay[0][0] === 'copy'
      && s.oldWay[0][1] === pkg.cmd && s.selection === pkg.cmd && s.state === 'selected' && /^Selected/.test(s.said);
  }
  await evaluate(`delete navigator.clipboard; getSelection().removeAllRanges(); true`);
  return c;
}

const clothD = () => evaluate(`document.getElementById('hero-cloth')?.getAttribute('d') || ''`);

async function run() {
  if (!port) {
    port = opt.port;
    if (await alive()) throw new Error(`port ${port} is already taken by another browser`);
    const base = realpathSync(opt.scratch || tmpdir());
    profile = mkdtempSync(join(base, 'pagecheck-'));
    const flags = ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}/prof`, '--no-first-run',
      '--no-default-browser-check', '--hide-scrollbars', '--mute-audio', '--disable-extensions', 'about:blank'];
    // its own process group, so a stuck browser can be stopped as a whole
    child = process.env.CHROME
      ? spawn(process.env.CHROME, flags, { stdio: 'ignore', detached: true })
      : spawn('flatpak', ['run', `--filesystem=${profile}`, 'com.google.Chrome', ...flags], { stdio: 'ignore', detached: true });
  }
  let version = null;
  for (let i = 0; i < 100 && !version; i++) { try { version = await json('/json/version'); } catch { await sleep(200); } }
  if (!version) throw new Error('no browser on port ' + port);
  browser = connect(version.webSocketDebuggerUrl);
  await browser.open;
  ({ targetId } = await browser.send('Target.createTarget', { url: 'about:blank' }));
  const targets = await json('/json');
  page = connect(targets.find(t => t.id === targetId).webSocketDebuggerUrl);
  await page.open;
  send = page.send;
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
  await send('Page.addScriptToEvaluateOnNewDocument', { source: CLIPBOARD_STUB });

  for (const w of WIDTHS) {
    const r = await load(w);
    if (opt.heroAt !== null) { await evaluate(`window.__clock = ${opt.heroAt * 1000}`); await sleep(400); }
    if (opt.shots) { mkdirSync(opt.shots, { recursive: true }); await shot(join(opt.shots, `w${w}.png`)); }
    out.widths.push({ width: r.width, innerWidth: r.innerWidth, scrollWidth: r.scrollWidth, fits: r.fits, stageScale: r.stageScale,
      stageBox: r.stageBox, button: r.button, buttonOk: !!r.button && r.button.box[3] >= 40, metaPx: r.metaPx,
      oneLiner: r.oneLiner, oneLinerOk: !r.oneLiner || (r.oneLiner.box[3] >= 40 && r.oneLiner.inView), aboveNext: r.aboveNext,
      copy: r.copy, copyOk: r.copy.buttons === r.copy.marked && !r.copy.bad.length && (w < 760 || r.copy.heroLines === null || r.copy.heroLines === 1),
      errors: log.errors.length });
    out.transferBytes[w] = log.bytes;
    out.consoleErrors.push(...log.errors.map(e => `${w}: ${e}`));
    out.foreignRequests.push(...log.foreign);
  }
  // the behaviour checks, at 1280 (and the zoom again at 390; the copy buttons at 1280, 390 and 320), on fresh loads
  if (opt.heroAt !== null) return;
  await load(1280);
  const a = await clothD(); await sleep(opt.reduced ? 2500 : 2200); const b = await clothD();
  const hand = await evaluate(`document.getElementById('hero-hand').style.opacity`);
  out.hero = opt.reduced ? { moves: a !== b, handOpacity: hand, hangs: a === b && a !== '' } : { moves: a !== b };
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
  out.copy = {};
  for (const w of [1280, 390, 320]) {
    await load(w);
    out.copy[w] = await copyCheck(w);
    out.consoleErrors.push(...log.errors.map(e => `checks: ${e}`));
    out.foreignRequests.push(...log.foreign);
  }
}

// Close what we opened: our tab; and a browser we launched — Browser.close over CDP first, then (if it is still
// there) its process group, then any process still running with our own fresh profile dir; then delete the profile.
let shut = null;
function shutdown() { return (shut ??= (async () => {
  log = null;
  if (page) { if (targetId && browser) await within(browser.send('Target.closeTarget', { targetId }).catch(() => {}), 2000); page.ws.close(); }
  if (child) {
    if (browser) await within(browser.send('Browser.close').catch(() => {}), 3000);
    const gone = async ms => { for (let t = 0; t < ms; t += 200) { if (!(await alive()) && !ours().length) return true; await sleep(200); } return false; };
    if (!(await gone(6000))) {
      for (const sig of ['SIGTERM', 'SIGKILL']) {
        try { process.kill(-child.pid, sig); } catch {}
        for (const pid of ours()) { try { process.kill(pid, sig); } catch {} }
        if (await gone(3000)) break;
      }
    }
    out.browserStopped = !(await alive()) && !ours().length;
  }
  if (browser) browser.ws.close();
  if (profile) rmSync(profile, { recursive: true, force: true });
})()); }
// the processes whose command line names our profile dir (unique per run: only ever ones this run started)
function ours() {
  if (!profile) return [];
  const mark = `--user-data-dir=${profile}/prof`, pids = [];
  for (const d of readdirSync('/proc')) {
    if (!/^\d+$/.test(d) || Number(d) === process.pid) continue;
    try { if (readFileSync(`/proc/${d}/cmdline`, 'utf8').split('\0').includes(mark)) pids.push(Number(d)); } catch {}
  }
  return pids;
}
process.on('SIGINT', () => shutdown().then(() => process.exit(130)));
process.on('SIGTERM', () => shutdown().then(() => process.exit(143)));

try {
  await run();
} catch (e) {
  out.fatal = String(e && e.message || e);
} finally {
  await shutdown();
}
out.foreignRequests = [...new Set(out.foreignRequests)];
const sizes = Object.values(out.transferBytes);
out.transferMB = sizes.length ? Math.round(Math.max(...sizes) / 1e5) / 10 : null;
const heroOk = !out.hero || (opt.reduced ? out.hero.hangs && out.hero.handOpacity === '0'
  : out.hero.moves && out.hero.pausedOffScreen && out.hero.resumesOnScreen);
const zoomOk = !out.zoom || Object.values(out.zoom).every(z => z && z.focusable && z.hint && z.opens && z.zoomFontPx >= 11
  && z.pageFits && z.pageScrollLocked && z.closesOnEsc && z.focusBack);
// the first screen's download button: a real button everywhere; off the scaled stage on phones, its links readable
const buttonOk = out.widths.every(w => w.buttonOk && (w.width >= 760 || (!w.button.inStage && w.metaPx >= 14))
  && w.oneLinerOk && w.aboveNext);
// the copy buttons: where the page has them, each check that found its button passed
const copyOk = out.widths.every(w => w.copyOk) && (!out.copy || Object.values(out.copy).every(c => Object.values(c).every(x => x.ok)));
out.ok = !out.fatal && out.widths.length === WIDTHS.length && out.widths.every(w => w.fits) && !out.consoleErrors.length
  && !out.foreignRequests.length && heroOk && zoomOk && buttonOk && copyOk && out.browserStopped !== false;
console.log(JSON.stringify(out, null, 1));
process.exit(out.ok ? 0 : 1);
