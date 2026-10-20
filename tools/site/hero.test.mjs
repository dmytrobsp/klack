// Unit tests for the headline's 6 fps loop (site/js/hero.js). Run: node --test "tools/site/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { FPS, LOOP, quantize, pose, peelShape, HANG_POSE, startHero, scrapPath } from '../../site/js/hero.js';

const W = 590, H = 132;
const close = (a, b, eps = 1e-9) => Math.abs(a - b) < eps;
const shape = p => [p.angle, p.len, p.wid, p.wave, p.taper, p.roll, p.printLen];

test('the clock: 6 fps, an 8.0 s loop', () => {
  assert.equal(FPS, 6);
  assert.equal(LOOP, 8.0);
  assert.equal(quantize(1.0, 6), 1.0);
  assert.equal(quantize(1.1, 6), 1.0);
  assert.ok(close(quantize(8.1, 6), 0.0, 1e-6), String(quantize(8.1, 6)));
  assert.ok(close(quantize(9.2, 6), 7 / 6, 1e-9), String(quantize(9.2, 6)));
});

test('the scrap sits still at the start', () => {
  const p = pose(0);
  assert.equal(p.phase, 'still');
  assert.equal(p.angle, 0);
  assert.equal(p.roll, 0);
  assert.equal(p.hand.visible, 0);
});

test('the phases follow the timeline', () => {
  assert.equal(pose(1.0).phase, 'still');
  assert.equal(pose(2.2).phase, 'peel');
  assert.ok(pose(2.2).peel.P > 0, 'the peel has started');
  assert.equal(pose(3.2).phase, 'fall');
  assert.equal(pose(4.25).phase, 'hang');
  assert.equal(pose(5.85).phase, 'slap');
  assert.equal(pose(7.0).phase, 'still');
});

test('it hangs limp at 4.25 s', () => {
  const a = pose(4.25).angle;
  assert.ok(a >= 80 && a <= 100, String(a));
});

test('the slap: flat again, the hand on it', () => {
  const p = pose(5.85);
  assert.equal(p.phase, 'slap');
  assert.equal(p.angle % 360, 0);
  assert.equal(p.hand.visible, 1);
  assert.equal(p.hand.follow, 1);
  assert.equal(p.slapping, true);
});

test('the loop closes: 7.9 s looks like 0 s', () => {
  assert.deepEqual(shape(pose(7.9)), shape(pose(0)));
  assert.equal(pose(7.9).hand.visible, 0);
});

test('pose is pure: the same t gives the same pose', () => {
  assert.deepEqual(pose(3.7), pose(3.7));
  assert.deepEqual(pose(3.7 + LOOP), pose(3.7));
});

test('peelShape before the peel: the whole rectangle, no roll', () => {
  const s = peelShape({ P: 0, R: 0, dang: 34, bulge: 0 }, W, H);
  assert.equal(s.roll, null);
  const xs = s.flat.map(p => p[0]), ys = s.flat.map(p => p[1]);
  assert.equal(Math.min(...xs), 0); assert.equal(Math.max(...xs), W);
  assert.equal(Math.min(...ys), 0); assert.equal(Math.max(...ys), H);
  for (const c of [[0, 0], [W, 0], [W, H], [0, H]])
    assert.ok(s.flat.some(p => p[0] === c[0] && p[1] === c[1]), 'corner ' + c);
});

test('peelShape mid-peel: the top-right corner goes first, the bottom-left stays', () => {
  const s = peelShape({ P: 215, R: 18, dang: 40, bulge: 15 }, W, H);
  assert.ok(!s.flat.some(p => p[0] === W && p[1] === 0), 'top-right corner is peeled');
  assert.ok(s.flat.some(p => p[0] === 0 && p[1] === H), 'bottom-left corner is flat');
  assert.ok(s.roll, 'a roll along the fold');
  assert.equal(s.roll.top.length, s.roll.bot.length);
  assert.ok(s.roll.top.length >= 2);
});

test('HANG_POSE is the scrap hanging at 4.25 s', () => {
  assert.ok(HANG_POSE.angle >= 80 && HANG_POSE.angle <= 100, String(HANG_POSE.angle));
  assert.equal(HANG_POSE.phase, 'hang');
  assert.deepEqual(HANG_POSE, pose(4.25));
});

// a minimal fake DOM: every element accepts attributes and styles
function fakeDom() {
  const els = new Map();
  const el = id => {
    if (!els.has(id)) els.set(id, {
      id, attrs: {}, style: {}, children: [0, 1, 2, 3].map(() => ({ attrs: {}, style: {}, setAttribute(k, v) { this.attrs[k] = String(v); } })),
      classList: { toggle() {} },
      setAttribute(k, v) { this.attrs[k] = String(v); },
    });
    return els.get(id);
  };
  const doc = {
    hidden: false,
    getElementById: el,
    listeners: {},
    addEventListener(type, fn) { doc.listeners[type] = fn; },
    removeEventListener(type) { delete doc.listeners[type]; },
  };
  const svg = { ownerDocument: doc, ...el('svg') };
  return { svg, doc, els };
}

test('startHero with reduced motion draws the hanging scrap once and schedules nothing', () => {
  const { svg, els } = fakeDom();
  const real = globalThis.setTimeout;
  let timers = 0;
  globalThis.setTimeout = () => { timers++; return 0; };
  try {
    const h = startHero(svg, { reduced: true });
    assert.equal(timers, 0);
    assert.equal(typeof h.stop, 'function');
    h.stop();
  } finally {
    globalThis.setTimeout = real;
  }
  const d = els.get('hero-cloth').attrs.d;
  assert.ok(d && d.startsWith('M'), 'the cloth is drawn');
  assert.equal(els.get('hero-hand').style.opacity, '0');
});

test('startHero runs one timer chain at 1000 / FPS and pauses off screen and in a hidden tab', () => {
  const { svg, doc } = fakeDom();
  const real = { st: globalThis.setTimeout, ct: globalThis.clearTimeout, io: globalThis.IntersectionObserver };
  const pending = new Map(); let next = 1, observer = null;
  globalThis.setTimeout = (fn, ms) => { assert.ok(close(ms, 1000 / FPS)); pending.set(next, fn); return next++; };
  globalThis.clearTimeout = id => { pending.delete(id); };
  globalThis.IntersectionObserver = class { constructor(cb) { observer = this; this.cb = cb; } observe() {} disconnect() { this.off = true; } };
  try {
    const h = startHero(svg, { reduced: false });
    assert.equal(pending.size, 1, 'one frame scheduled');
    const [[id, fn]] = pending; pending.delete(id); fn();
    assert.equal(pending.size, 1, 'each frame schedules exactly the next one');
    observer.cb([{ isIntersecting: false }]);
    assert.equal(pending.size, 0, 'off screen: no timer');
    observer.cb([{ isIntersecting: true }]);
    assert.equal(pending.size, 1, 'back on screen: running again');
    doc.hidden = true; doc.listeners.visibilitychange();
    assert.equal(pending.size, 0, 'hidden tab: no timer');
    doc.hidden = false; doc.listeners.visibilitychange();
    assert.equal(pending.size, 1);
    h.stop();
    assert.equal(pending.size, 0, 'stopped');
    assert.ok(observer.off, 'the observer is disconnected');
    assert.equal(doc.listeners.visibilitychange, undefined);
  } finally {
    globalThis.setTimeout = real.st; globalThis.clearTimeout = real.ct; globalThis.IntersectionObserver = real.io;
  }
});

test('without JavaScript the page already shows the scrap flat over the word', () => {
  const html = readFileSync(new URL('../../site/index.html', import.meta.url), 'utf8');
  const d = scrapPath(pose(0));
  assert.ok(html.includes(`id="hero-cloth" d="${d}"`), 'the cloth is drawn in the markup');
  assert.ok(html.includes(`id="hero-clip" d="${d}"`), 'and so is its clip');
});
