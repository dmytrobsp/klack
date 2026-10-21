// The terminal captures, readable: in their slots a capture is ~110 columns in ~330–400 px (5–6 px a
// character), so a click, a tap, or Enter/Space on the focused capture opens it enlarged in a modal
// <dialog>; Esc, a click anywhere or the × closes it and focus goes back. Where the screen is too narrow
// for a readable size (phones), the enlarged capture scrolls sideways inside the dialog.
// No network, no third-party code. The sizing is pure (tested in tools/site/zoom.test.mjs).

export const CH = 0.6125;   // Space Mono: one character is 0.6125 em wide (as in .capture .grid's font-size)
export const PAD = 20;      // .capture .screen's left + right padding, px

/**
 * The enlarged capture's width (px) for `cols` columns when `avail` px are free: as wide as fits, but the
 * characters stay between `min` and `max` px tall. `scrolls`: wider than `avail` (the dialog scrolls).
 */
export function zoomWidth(cols, avail, { min = 12, max = 16 } = {}) {
  const lo = Math.ceil(cols * CH * min + PAD), hi = Math.floor(cols * CH * max + PAD);
  const width = Math.max(lo, Math.min(hi, Math.floor(avail)));
  return { width, fontPx: (width - PAD) / cols / CH, scrolls: width > avail };
}

/** The keys that open a focused capture. */
export const opensOn = key => key === 'Enter' || key === ' ' || key === 'Spacebar';

export const hint = touch => (touch ? 'tap to zoom' : 'click to zoom');

export function enableZoom(doc = document) {
  const win = doc.defaultView, caps = [...doc.querySelectorAll('.term.capture')];
  if (!caps.length || typeof doc.createElement('dialog').showModal !== 'function') return null;
  const touch = !!(win.matchMedia && win.matchMedia('(pointer: coarse)').matches);
  const dlg = doc.createElement('dialog');
  dlg.className = 'zoom';
  dlg.innerHTML = '<button class="zoom-close" type="button" aria-label="Close">×</button><div class="zoom-body"></div>';
  doc.body.append(dlg);
  const body = dlg.querySelector('.zoom-body');
  let from = null;

  function open(cap) {
    const copy = cap.cloneNode(true);
    copy.querySelector('.zoom-hint')?.remove();
    copy.removeAttribute('tabindex'); copy.removeAttribute('aria-haspopup');
    copy.setAttribute('role', 'img'); copy.setAttribute('aria-label', cap.dataset.label || '');
    const cols = parseFloat(cap.style.getPropertyValue('--cols')) || 110;
    const avail = Math.min(win.innerWidth, doc.documentElement.clientWidth || win.innerWidth) - 32;
    copy.style.width = zoomWidth(cols, avail).width + 'px';
    body.replaceChildren(copy);
    dlg.setAttribute('aria-label', cap.dataset.label || 'capture');
    from = cap;
    dlg.showModal();
  }
  dlg.addEventListener('click', () => dlg.close());   // anywhere, the capture included (a sideways swipe is no click)
  dlg.addEventListener('close', () => {
    body.replaceChildren();
    if (from) from.focus({ preventScroll: true });
    from = null;
  });

  for (const cap of caps) {
    const label = cap.getAttribute('aria-label') || '';
    cap.dataset.label = label;
    cap.tabIndex = 0;
    cap.setAttribute('role', 'button');
    cap.setAttribute('aria-haspopup', 'dialog');
    cap.setAttribute('aria-label', `${label} (${hint(touch)})`);
    const h = doc.createElement('span');
    h.className = 'zoom-hint'; h.setAttribute('aria-hidden', 'true'); h.textContent = hint(touch);
    cap.querySelector('.dots')?.append(h);
    cap.addEventListener('click', () => open(cap));
    cap.addEventListener('keydown', e => { if (opensOn(e.key)) { e.preventDefault(); open(cap); } });
  }
  return dlg;
}
