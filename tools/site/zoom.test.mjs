// Unit tests for the captures' zoom (site/js/zoom.js). Run: node --test "tools/site/*.test.mjs"
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { CH, PAD, zoomWidth, opensOn, hint } from '../../site/js/zoom.js';

test('the stylesheet agrees with the zoom on the character width and the padding', () => {
  const css = readFileSync(new URL('../../site/css/site.css', import.meta.url), 'utf8');
  assert.ok(css.includes(`font-size: calc(100cqw / var(--cols) / ${CH})`), 'the grid scales with the same ch/em');
  const pad = css.match(/\.capture \.screen \{[^}]*padding: (\d+)px (\d+)px/);
  assert.equal(Number(pad[2]) * 2, PAD);
});

test('zoomWidth: a desktop gets the capture as wide as fits, capped at 16 px characters', () => {
  const z = zoomWidth(110, 1248);
  assert.equal(z.width, Math.floor(110 * CH * 16 + PAD));
  assert.ok(z.fontPx <= 16 && z.fontPx > 15.9, String(z.fontPx));
  assert.equal(z.scrolls, false);
  const mid = zoomWidth(110, 1000);
  assert.equal(mid.width, 1000);
  assert.ok(mid.fontPx > 12 && mid.fontPx < 16);
  assert.equal(mid.scrolls, false);
});

test('zoomWidth: a phone keeps 12 px characters and scrolls sideways', () => {
  const z = zoomWidth(110, 358);
  assert.ok(z.fontPx >= 12, String(z.fontPx));
  assert.ok(z.width > 358);
  assert.equal(z.scrolls, true);
  assert.ok(zoomWidth(80, 358).width < z.width, 'fewer columns, narrower');
});

test('Enter and Space open a focused capture; other keys do not', () => {
  assert.ok(opensOn('Enter'));
  assert.ok(opensOn(' '));
  assert.ok(!opensOn('Escape'));
  assert.ok(!opensOn('Tab'));
  assert.ok(!opensOn('a'));
});

test('the hint says tap on touch screens and click elsewhere', () => {
  assert.equal(hint(true), 'tap to zoom');
  assert.equal(hint(false), 'click to zoom');
});
