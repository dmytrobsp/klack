// Unit tests for the install commands' copy buttons (site/js/copy.js). Run: node --test "tools/site/*.test.mjs"
// No DOM in node: a few fake nodes stand in for the page, and no test ever reaches a real clipboard.
import { test, mock } from 'node:test';
import assert from 'node:assert/strict';
import { RESET_MS, commandText, copyText, show, addCopyButtons } from '../../site/js/copy.js';

// ---- a fake page: elements, text nodes, the few calls copy.js makes ----
class Text { constructor(data) { this.nodeType = 3; this.data = data; } }
class El {
  constructor(tag, attrs = {}, kids = []) {
    this.nodeType = 1; this.tagName = tag.toUpperCase(); this.attrs = { ...attrs }; this.childNodes = [];
    this.dataset = {}; this.listeners = {}; this.innerHTML = '';
    const cls = new Set((attrs.class || '').split(' ').filter(Boolean));
    this.classList = { contains: c => cls.has(c), add: c => cls.add(c) };
    Object.defineProperty(this, 'className', { set: v => { cls.clear(); v.split(' ').forEach(c => cls.add(c)); } });
    for (const k of kids) this.append(typeof k === 'string' ? new Text(k) : k);
  }
  append(n) { n.parentNode = this; this.childNodes.push(n); }
  prepend(n) { n.parentNode = this; this.childNodes.unshift(n); }
  setAttribute(k, v) { this.attrs[k] = String(v); }
  getAttribute(k) { return k in this.attrs ? this.attrs[k] : null; }
  addEventListener(type, f) { (this.listeners[type] ||= []).push(f); }
  set textContent(v) { this.childNodes = [new Text(v)]; }
  get textContent() { return this.childNodes.map(n => (n.nodeType === 3 ? n.data : n.textContent)).join(''); }
  all() { return this.childNodes.filter(n => n.nodeType === 1).flatMap(n => [n, ...n.all()]); }
  matches(sel) {
    if (sel.startsWith('.')) return this.classList.contains(sel.slice(1));
    if (sel.startsWith('[')) return sel.slice(1, -1) in this.attrs;
    return false;
  }
  querySelectorAll(sel) { return this.all().filter(e => e.matches(sel)); }
  querySelector(sel) { return this.querySelectorAll(sel)[0] || null; }
  async click() { for (const f of this.listeners.click || []) await f(); }
}
const ps = p => new El('span', { class: 'ps' }, [p]);

function fakePage(nav = {}) {
  const hero = new El('code', { class: 'dl-cmd', 'data-copy': '' }, [ps('$'), ' curl -fsSL https://ttyak.sh/install.sh | sh']);
  const win = new El('pre', { class: 'cmd', 'data-copy': '' }, [ps('>'), ' irm https://ttyak.sh/install.ps1 | iex  ']);
  const plain = new El('code', { 'data-copy': '' }, ['scoop install ttyak']);   // inline, in the package list
  const other = new El('code', {}, ['ttyak']);   // not a command: no button
  const body = new El('body', {}, [hero, win, new El('li', {}, [plain, ' ', other])]);
  const doc = {
    body, selected: null, copiedTheOldWay: [],
    defaultView: { navigator: nav, getComputedStyle: el => ({ display: el === plain ? 'inline' : 'block' }) },
    querySelectorAll: s => body.querySelectorAll(s), querySelector: s => body.querySelector(s),
    createElement: tag => new El(tag),
    createRange: () => ({ setStart(n, o) { this.start = [n, o]; }, setEnd(n, o) { this.end = [n, o]; } }),
    getSelection: () => ({ removeAllRanges() {}, addRange: r => { doc.selected = r; } }),
    execCommand(c) { doc.copiedTheOldWay.push(c); return doc.oldWayWorks === true; },
  };
  return { doc, hero, win, plain, other };
}
const selectedText = r => {
  const [n, o] = r.start, [m, p] = r.end;
  assert.equal(n, m, 'one text node here');
  return n.data.slice(o, p);
};
const fakeClipboard = () => {
  const got = [];
  return { got, writeText: async t => { got.push(t); } };
};

// ---- the text ----
test('commandText: the command without its prompt, and without spaces around it', () => {
  const { hero, win, plain } = fakePage();
  assert.equal(commandText(hero), 'curl -fsSL https://ttyak.sh/install.sh | sh');
  assert.equal(commandText(win), 'irm https://ttyak.sh/install.ps1 | iex');
  assert.equal(commandText(plain), 'scoop install ttyak');
});

test('commandText: the button inside the command is not part of it', () => {
  const { doc, hero } = fakePage();
  addCopyButtons(doc);
  hero.querySelector('.copy').textContent = 'Copied';
  assert.equal(commandText(hero), 'curl -fsSL https://ttyak.sh/install.sh | sh');
});

// ---- copying ----
test('copyText: the clipboard API when there is one', async () => {
  const clip = fakeClipboard();
  let fellBack = false;
  assert.equal(await copyText('choco install ttyak', clip, () => (fellBack = true)), 'copied');
  assert.deepEqual(clip.got, ['choco install ttyak']);
  assert.equal(fellBack, false);
});

test('copyText: the fallback without a clipboard API (http, old browsers), or when it refuses', async () => {
  for (const clip of [undefined, {}, { writeText: async () => { throw new Error('NotAllowedError'); } }]) {
    let calls = 0;
    assert.equal(await copyText('x', clip, () => { calls++; return true; }), 'copied');
    assert.equal(await copyText('x', clip, () => { calls++; return false; }), 'selected');
    assert.equal(calls, 2);
  }
});

test('a click copies the command exactly, through the clipboard API', async () => {
  const clip = fakeClipboard();
  const { doc, hero, win } = fakePage({ clipboard: clip });
  addCopyButtons(doc);
  await hero.querySelector('.copy').click();
  await win.querySelector('.copy').click();
  assert.deepEqual(clip.got, ['curl -fsSL https://ttyak.sh/install.sh | sh', 'irm https://ttyak.sh/install.ps1 | iex']);
  assert.deepEqual(doc.copiedTheOldWay, []);
  assert.equal(hero.querySelector('.copy').dataset.state, 'copied');
  assert.equal(doc.querySelector('.copy-status').textContent, 'Copied');
});

test('without the clipboard API a click selects the command (no prompt, no spaces) and copies it the old way', async () => {
  for (const works of [true, false]) {
    const { doc, win } = fakePage({});
    doc.oldWayWorks = works;
    addCopyButtons(doc);
    await win.querySelector('.copy').click();
    assert.equal(selectedText(doc.selected), 'irm https://ttyak.sh/install.ps1 | iex');
    assert.deepEqual(doc.copiedTheOldWay, ['copy']);
    assert.equal(win.querySelector('.copy').dataset.state, works ? 'copied' : 'selected');
    assert.match(doc.querySelector('.copy-status').textContent, works ? /^Copied$/ : /^Selected/);
  }
});

// ---- the buttons ----
test('addCopyButtons: one button inside each marked command, none elsewhere, and only once', () => {
  const { doc, hero, win, plain, other } = fakePage();
  assert.equal(addCopyButtons(doc), 3);
  for (const el of [hero, win, plain]) {
    const b = el.querySelectorAll('.copy');
    assert.equal(b.length, 1);
    assert.equal(b[0].tagName, 'BUTTON');
    assert.equal(b[0].type, 'button');
    assert.equal(b[0].getAttribute('aria-label'), `Copy ${commandText(el)}`);
  }
  // first in a box (floated to its first line's end by the CSS), after the text of an inline command
  assert.ok(hero.childNodes[0].classList.contains('copy') && win.childNodes[0].classList.contains('copy'));
  assert.ok(plain.childNodes.at(-1).classList.contains('copy'));
  assert.equal(other.querySelector('.copy'), null);
  assert.equal(addCopyButtons(doc), 0, 'a second run (after download.js rewrote the hero) adds none twice');
  assert.equal(hero.querySelectorAll('.copy').length, 1);
});

test('addCopyButtons: one live region for the whole page, said to screen readers', () => {
  const { doc } = fakePage();
  addCopyButtons(doc);
  addCopyButtons(doc);
  const live = doc.body.querySelectorAll('.copy-status');
  assert.equal(live.length, 1);
  assert.equal(live[0].getAttribute('role'), 'status');
  assert.equal(live[0].getAttribute('aria-live'), 'polite');
});

test('addCopyButtons: a page without marked commands (before 0.11.0) gets nothing', () => {
  const body = new El('body', {}, [new El('pre', { class: 'cmd' }, [ps('$'), ' ttyak auth login'])]);
  const doc = { body, querySelectorAll: s => body.querySelectorAll(s), querySelector: s => body.querySelector(s),
    createElement: () => assert.fail('nothing is made') };
  assert.equal(addCopyButtons(doc), 0);
  assert.equal(body.querySelector('.copy-status'), null);
});

// ---- the feedback ----
test('show: the feedback resets after RESET_MS, and a second click starts it over', () => {
  mock.timers.enable({ apis: ['setTimeout'] });
  try {
    const { doc, hero } = fakePage();
    addCopyButtons(doc);
    const b = hero.querySelector('.copy'), live = doc.querySelector('.copy-status'), idle = b.innerHTML;
    assert.match(idle, /^<svg/);
    show(b, live, 'copied');
    assert.equal(b.dataset.state, 'copied');
    assert.equal(live.textContent, 'Copied');
    assert.notEqual(b.innerHTML, idle, 'a check instead of the copy icon');
    mock.timers.tick(RESET_MS - 100);
    show(b, live, 'copied');
    mock.timers.tick(RESET_MS - 100);
    assert.equal(b.dataset.state, 'copied', 'the second click keeps it a full RESET_MS');
    mock.timers.tick(100);
    assert.equal(b.dataset.state, undefined);
    assert.equal(b.innerHTML, idle);
    assert.equal(live.textContent, '');
  } finally {
    mock.timers.reset();
  }
});

test('RESET_MS is about one and a half seconds', () => {
  assert.equal(RESET_MS, 1500);
});
