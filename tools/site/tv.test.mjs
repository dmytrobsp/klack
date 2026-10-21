// Unit tests for the huddle capture's TV (site/js/tv.js, site.css). Run: node --test "tools/site/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { pauseOffScreen } from '../../site/js/tv.js';

const css = readFileSync(new URL('../../site/css/site.css', import.meta.url), 'utf8');

function fakeTv() {
  const set = new Set();
  return { classList: { add: c => set.add(c), toggle: (c, on) => (on ? set.add(c) : set.delete(c)), has: c => set.has(c) } };
}

test('the TV is paused off screen and runs on screen', () => {
  const tvs = [fakeTv(), fakeTv()];
  let cb, watched = [];
  class IO { constructor(f) { cb = f; } observe(t) { watched.push(t); } }
  const doc = { querySelectorAll: s => (s === '.capture .tv' ? tvs : []) };
  assert.ok(pauseOffScreen(doc, IO));
  assert.deepEqual(watched, tvs);
  assert.ok(tvs.every(t => t.classList.has('paused')), 'paused until seen');
  cb([{ target: tvs[0], isIntersecting: true }]);
  assert.ok(!tvs[0].classList.has('paused'));
  assert.ok(tvs[1].classList.has('paused'));
  cb([{ target: tvs[0], isIntersecting: false }]);
  assert.ok(tvs[0].classList.has('paused'));
});

test('nothing to do without a TV or without IntersectionObserver', () => {
  assert.equal(pauseOffScreen({ querySelectorAll: () => [] }, class {}), null);
  assert.equal(pauseOffScreen({ querySelectorAll: () => [fakeTv()] }, undefined), null);
});

test('the stylesheet pauses a paused TV, keeps the zoomed one running, and cuts hard', () => {
  assert.match(css, /\.capture \.tv\.paused img, \.capture \.tv\.paused::after \{ animation-play-state: paused; \}/);
  assert.match(css, /dialog\.zoom \.capture \.tv img, dialog\.zoom \.capture \.tv::after \{ animation-play-state: running; \}/);
  assert.match(css, /animation: tv-swan 16s step-end infinite/);
  assert.match(css, /@keyframes tv-swan \{ 0% \{ opacity: 1; \} 25%, 100% \{ opacity: 0; \} \}/);
  assert.match(css, /prefers-reduced-motion: reduce\) \{ \.capture \.tv img, \.capture \.tv::after \{ animation: none; \}/);
});
