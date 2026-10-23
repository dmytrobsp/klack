// The install commands' copy buttons. fill.py and download.js mark each command: `data-copy` on its <code> or <pre>,
// the prompt (`$` or `>`) in a <span class="ps">. addCopyButtons() puts a button inside each one that copies the
// command without its prompt: first in a box (the CSS floats it to the right end of the first line, so the command
// never wraps more for it), after the text of an inline one. With JS off there are no buttons and the commands read as
// they are.
// The clipboard API where there is one (https, localhost); without it (http, an old browser) or when it refuses, the
// command's text is selected and copied the old way (execCommand) where that still works. A screen reader hears what
// happened from one live region. No network, no third-party code. Tested in tools/site/copy.test.mjs.

export const RESET_MS = 1500;   // how long a button shows what happened

const svg = d => `<svg viewBox="0 0 16 16" aria-hidden="true" focusable="false"><path d="${d}" fill="none" `
  + 'stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>';
const ICON = {
  idle: svg('M6.5 5.5H13a1 1 0 0 1 1 1V13a1 1 0 0 1-1 1H6.5a1 1 0 0 1-1-1V6.5a1 1 0 0 1 1-1ZM10.5 5.5V3a1 1 0 0 0-1-1H3a1 1 0 0 0-1 1v6.5a1 1 0 0 0 1 1h2.5'),
  copied: svg('M3 8.5l3.2 3.2L13 4.8'),
};
ICON.selected = ICON.idle;   // the selection on the page is the sign
const SAY = { copied: 'Copied', selected: 'Selected: copy it with Ctrl+C (⌘C on a Mac)' };

const left = n => n.nodeType === 1 && (n.classList.contains('ps') || n.classList.contains('copy'));

function texts(el, out = []) {
  for (const n of el.childNodes) {
    if (n.nodeType === 3) out.push(n);
    else if (n.nodeType === 1 && !left(n)) texts(n, out);
  }
  return out;
}

/** The command an element shows: its text without the prompt and the button, trimmed. */
export const commandText = el => texts(el).map(n => n.data).join('').trim();

/** Selects the command on the page (not the prompt, the spaces or the button) and copies it the old way: true if copied. */
function selectCommand(el, doc) {
  const ns = texts(el).filter(n => n.data.trim());
  if (!ns.length) return false;
  const first = ns[0], last = ns[ns.length - 1];
  const range = doc.createRange();
  range.setStart(first, first.data.length - first.data.trimStart().length);
  range.setEnd(last, last.data.trimEnd().length);
  const sel = doc.getSelection();
  sel.removeAllRanges();
  sel.addRange(range);
  try { return doc.execCommand('copy') === true; } catch { return false; }
}

/** Copies `text`: 'copied' through the clipboard API; else what `fallback` managed, 'copied' or 'selected'. */
export async function copyText(text, clipboard, fallback) {
  if (clipboard && typeof clipboard.writeText === 'function') {
    try { await clipboard.writeText(text); return 'copied'; } catch { /* refused: select it instead */ }
  }
  return fallback() ? 'copied' : 'selected';
}

const timers = new WeakMap();

/** Shows `state` on the button and says it in the live region; back to the copy icon after RESET_MS. */
export function show(button, live, state) {
  clearTimeout(timers.get(button));
  button.dataset.state = state;
  button.innerHTML = ICON[state];
  live.textContent = SAY[state];
  timers.set(button, setTimeout(() => {
    delete button.dataset.state;
    button.innerHTML = ICON.idle;
    if (live.textContent === SAY[state]) live.textContent = '';
  }, RESET_MS));
}

/** Puts a copy button in each marked command that has none yet (download.js may have rewritten the hero); how many. */
export function addCopyButtons(doc = document) {
  const win = doc.defaultView;
  const marked = [...doc.querySelectorAll('[data-copy]')].filter(el => !el.querySelector('.copy'));
  if (!marked.length) return 0;
  let live = doc.querySelector('.copy-status');
  if (!live) {
    live = doc.createElement('span');
    live.className = 'copy-status';
    live.setAttribute('role', 'status');
    live.setAttribute('aria-live', 'polite');
    doc.body.append(live);
  }
  for (const el of marked) {
    const b = doc.createElement('button');
    b.type = 'button';
    b.className = 'copy';
    b.title = 'Copy';
    b.setAttribute('aria-label', `Copy ${commandText(el)}`);
    b.innerHTML = ICON.idle;
    b.addEventListener('click', async () => {
      show(b, live, await copyText(commandText(el), win && win.navigator && win.navigator.clipboard, () => selectCommand(el, doc)));
    });
    if (win && win.getComputedStyle(el).display === 'inline') el.append(b);
    else el.prepend(b);
  }
  return marked.length;
}
