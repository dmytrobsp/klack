// The headline's loop: the "BROWSER." scrap peels off "BULLSHIT.", hangs limp, and a hand slaps it back.
// Stop-motion at 6 fps, 8.0 s loop. Ported from the approved mockup (hero-v17). The timeline and the
// shapes are pure functions (tested in tools/site/hero.test.mjs: node --test "tools/site/*.test.mjs");
// only startHero touches the DOM.

export const FPS = 6, LOOP = 8.0;

// The flat scrap on the stage: its top-left corner is taped to the wall (the pin).
const PX = 140, PY = 168, W = 590, H = 132;

// t, angle° about the pin (90 = hanging straight down), flat length, width, wave, taper,
// hand x, hand y (when free), follow (1 = riding the scrap), hand visible, hand rotation°,
// roll radius (the curled-up end), print length (how the printing squashes as it hangs)
const K = [
  [0.0,    0, 1,   1,   0, 0,    -200, 520, 0, 0,  0,   0, 1],
  [1.5,    0, 1,   1,   0, 0,    -200, 520, 0, 0,  0,   0, 1],
  [3.0,   12, .2,  .98, 4, .05,  -200, 520, 0, 0,  0,  26, .55],  // (after the peel) the roll at the tape
  [3.15,  30, .18, .97, 6, .1,   -200, 520, 0, 0,  0,  22, .5],   // drops…
  [3.35,  72, .32, .95, 12, .3,  -200, 520, 0, 0,  0,  10, .45],  // …unrolls…
  [3.55, 102, .45, .95, 22, .45, -200, 520, 0, 0,  0,   0, .46],  // …and goes limp
  [3.85,  82, .44, .95, 18, .5,  -200, 520, 0, 0,  0,   0, .46],
  [4.25,  93, .44, .92, 16, .5,  -200, 520, 0, 0,  0,   0, .46],
  [4.75,  87, .44, .92, 17, .5,  -200, 520, 0, 0,  0,   0, .46],
  [5.0,   90, .44, .92, 16, .5,  -120, 470, 0, 1, 20,   0, .46],  // the hand comes up from below
  [5.3,   90, .44, .92, 16, .5,  -120, 470, 1, 1, 10,   0, .46],  // …catches it
  [5.6,   42, .72, .97, 10, .25, -120, 470, 1, 1, -5,   0, .75],  // …sweeps it back
  [5.8,    0, 1,   1,    0, 0,   -120, 470, 1, 1,  0,   0, 1],    // SLAP
  [6.05,   0, 1,   1,    0, 0,   -120, 470, 1, 1,  0,   0, 1],
  [6.3,    0, 1,   1,    0, 0,   -160, 560, 0, 1, 25,   0, 1],    // gone
  [6.5,    0, 1,   1,    0, 0,   -160, 560, 0, 0, 25,   0, 1],
  [8.0,    0, 1,   1,    0, 0,   -200, 520, 0, 0,  0,   0, 1],
];
// The peel (1.5 s – 3.0 s): the wind lifts the top-right corner and the curl travels down and to the left.
// t, peel distance from the corner, roll radius, peel direction° below "straight left", fold bulge
const KP = [
  [1.5,    0,  0, 34,  0],
  [1.7,   16,  5, 46,  3],
  [1.9,   70, 10, 38,  8],
  [2.1,  140, 14, 34, 12],
  [2.3,  215, 18, 40, 15],
  [2.5,  290, 21, 35, 13],
  [2.7,  360, 24, 39, 15],
  [2.9,  415, 27, 36, 11],
  [3.0,  430, 27, 37, 11],
];
const PEEL0 = 1.5, PEEL1 = 3.0, FALL1 = 3.55, CATCH = 5.3, GONE = 6.5, SLAP0 = 5.8, SLAP1 = 6.05;

const lerp = (a, b, u) => a + (b - a) * u;
const wrap = t => ((t % LOOP) + LOOP) % LOOP;
// smoothstep between keyframes; column `step` (if any) switches at the half instead of blending
function atK(T, t, step = -1) {
  let i = 0; while (i < T.length - 2 && T[i + 1][0] <= t) i++;
  const a = T[i], b = T[i + 1], u = Math.min(1, Math.max(0, (t - a[0]) / (b[0] - a[0] || 1)));
  const e = u * u * (3 - 2 * u);
  return a.map((v, j) => j === step ? (u < .5 ? a[j] : b[j]) : lerp(v, b[j], e));
}
// a fixed per-point jitter, so edges are hand-drawn but don't boil
const noise = (i, k) => Math.sin(i * 12.9898 + k * 78.233) * 43758.5453 % 1;
const f1 = v => v.toFixed(1);
function jag(P) {
  return 'M' + P.map(([x, y], i) => `${f1(x + (noise(i, 7) - .5) * 3)} ${f1(y + (noise(i, 8) - .5) * 3)}`).join(' L') + 'Z';
}
function smooth(P) {
  const n = P.length; let d = `M${f1(P[0][0])} ${f1(P[0][1])}`;
  for (let i = 0; i < n; i++) {
    const p0 = P[(i - 1 + n) % n], p1 = P[i], p2 = P[(i + 1) % n], p3 = P[(i + 2) % n];
    d += ` C${f1(p1[0] + (p2[0] - p0[0]) / 6)} ${f1(p1[1] + (p2[1] - p0[1]) / 6)} ${f1(p2[0] - (p3[0] - p1[0]) / 6)} ${f1(p2[1] - (p3[1] - p1[1]) / 6)} ${f1(p2[0])} ${f1(p2[1])}`;
  }
  return d + 'Z';
}

/** Stage time, looped and snapped to whole frames: the choppy stop-motion look. */
export function quantize(t, fps) {
  return Math.floor(wrap(t) * fps) / fps;
}

/** The peel's flat part and its roll, in paper-local coordinates (0,0 = the taped corner). */
export function peelShape({ P, R, dang, bulge }, W, H) {
  const a = dang * Math.PI / 180, dx = -Math.cos(a), dy = Math.sin(a);   // from the corner, left and down
  const TR = [W, 0];
  const side = ([x, y]) => (x - TR[0]) * dx + (y - TR[1]) * dy - P;      // ≥ 0: still flat on the wall
  // the paper's outline, edges split into short steps so the clip and the wobble have points to work with
  const rect = [[0, 0], [W, 0], [W, H], [0, H]], pts = [];
  for (let i = 0; i < 4; i++) {
    const [x0, y0] = rect[i], [x1, y1] = rect[(i + 1) % 4], n = Math.max(2, Math.round(Math.hypot(x1 - x0, y1 - y0) / 28));
    for (let k = 0; k < n; k++) pts.push([x0 + (x1 - x0) * k / n, y0 + (y1 - y0) * k / n]);
  }
  // keep the flat part (Sutherland–Hodgman against the fold line)
  const flat = [];
  for (let i = 0; i < pts.length; i++) {
    const p = pts[i], q = pts[(i + 1) % pts.length], sp = side(p), sq = side(q);
    if (sp >= 0) flat.push(p);
    if ((sp >= 0) !== (sq >= 0)) { const u = sp / (sp - sq); flat.push([p[0] + (q[0] - p[0]) * u, p[1] + (q[1] - p[1]) * u]); }
  }
  // the roll: along the fold line, on the peeled side, lumpy and thicker in the middle
  let roll = null;
  if (R > 1 && P > 4) {
    const nx = -dy, ny = dx, c = [TR[0] + dx * P, TR[1] + dy * P], hits = [];
    [[0, 0, W, 0], [W, 0, W, H], [W, H, 0, H], [0, H, 0, 0]].forEach(([x0, y0, x1, y1]) => {
      const ex = x1 - x0, ey = y1 - y0, den = ex * ny - ey * nx;
      if (Math.abs(den) < 1e-6) return;
      const u = ((c[0] - x0) * ny - (c[1] - y0) * nx) / den;
      if (u >= 0 && u <= 1) hits.push([x0 + ex * u, y0 + ey * u]);
    });
    if (hits.length >= 2) {
      let [A, B] = hits; if (A[1] > B[1]) [A, B] = [B, A];
      const top = [], bot = [], N = 9;
      for (let k = 0; k <= N; k++) {
        const s = k / N, bx = A[0] + (B[0] - A[0]) * s, by = A[1] + (B[1] - A[1]) * s;
        const bend = bulge * Math.sin(Math.PI * s);                         // the fold isn't straight
        const r = R * (.8 + .35 * Math.sin(Math.PI * s) + (noise(k, 3) - .5) * .35);
        const cx = bx - dx * (R * .7 + bend), cy = by - dy * (R * .7 + bend);
        top.push([cx + dx * r, cy + dy * r]); bot.push([cx - dx * r, cy - dy * r]);
      }
      roll = { top, bot };   // index-aligned: top[k] and bot[k] face each other across the roll
    }
  }
  return { flat, roll };
}

// A point of the hanging scrap on the stage: u along its length (0 = taped end), v across (0 = top edge).
function pt(p, u, v) {
  const { angle, len, wid, wave, taper } = p, phase = p.t * 9;
  let x = u * W * len;
  let y = v * H * wid * (1 - taper * u) + wave * Math.sin(u * Math.PI * 1.6 + phase) * u
        + wave * .45 * Math.sin(u * Math.PI * 4.4 + phase * 1.3 + v * 2.2) * u;
  x -= taper * 40 * Math.sin(v * Math.PI) * u;   // the free end sags into a lump
  const r = angle * Math.PI / 180, c = Math.cos(r), sn = Math.sin(r);
  return [PX + x * c - y * sn, PY + x * sn + y * c];
}

/**
 * The scrap at stage time t (seconds; looped). `jitter` (degrees) nudges the angle while it moves —
 * the drawing passes a small random value per frame, tests pass none.
 */
export function pose(t, jitter = 0) {
  t = wrap(t);
  if (t >= PEEL0 && t < PEEL1) {
    const [, P, R, dang, bulge] = atK(KP, t);
    return {
      t, phase: 'peel', angle: 0, len: 1, wid: 1, wave: 0, taper: 0, roll: 0, printLen: 1,
      peel: { P, R, dang, bulge }, slapping: false,
      hand: { x: -200, y: 520, rot: 0, visible: 0, follow: 0 },
    };
  }
  const s = atK(K, t, 9);
  const moving = !(s[1] === 0 && s[4] === 0 && s[11] === 0);
  const phase = t < PEEL0 ? 'still' : t < FALL1 ? 'fall' : t < CATCH ? 'hang' : t < GONE ? 'slap' : 'still';
  const p = {
    t, phase, angle: s[1] + (moving ? jitter : 0), len: s[2], wid: s[3], wave: s[4], taper: s[5],
    roll: s[11], printLen: s[12], peel: null, slapping: t >= SLAP0 && t < SLAP1,
  };
  const [cx, cy] = pt(p, .62, .55);
  p.hand = { x: lerp(s[6], cx, s[8]), y: lerp(s[7], cy, s[8]), rot: s[10], visible: s[9], follow: s[8] };
  return p;
}

/** The pose at 4.25 s: hanging limp from the tape. Shown, still, under prefers-reduced-motion. */
export const HANG_POSE = Object.freeze(pose(4.25));

/** The outline of the scrap's paper (the `d` of the cloth and its clip) on the stage. */
export function scrapPath(p) {
  if (p.peel) {
    const { flat } = peelShape(p.peel, W, H);
    return flat.length > 2
      ? jag(flat.map(([x, y], i) => [PX + x + (noise(i, 1) - .5) * 2.5, PY + y + (noise(i, 2) - .5) * 2.5]))
      : '';
  }
  const P = [];
  for (let i = 0; i <= 20; i++) P.push(pt(p, i / 20, 0));
  for (let i = 1; i <= 4; i++) P.push(pt(p, 1, i / 4));
  for (let i = 19; i >= 0; i--) P.push(pt(p, i / 20, 1));
  for (let i = 3; i >= 1; i--) P.push(pt(p, 0, i / 4));
  return jag(P);
}

const line = P => 'M' + P.map(p => f1(p[0]) + ' ' + f1(p[1])).join(' L');
const mid = (a, b, w) => a.map((p, i) => [p[0] + (b[i][0] - p[0]) * w, p[1] + (b[i][1] - p[1]) * w]);

// Draw one pose into the SVG layer (ids hero-*), and flicker the neon unless told not to.
function draw(el, p, flicker) {
  const d = scrapPath(p);
  el.cloth.setAttribute('d', d); el.clip.setAttribute('d', d);
  if (p.peel) {
    for (const f of el.folds) f.style.opacity = '0';
    el.print.setAttribute('transform', '');
    el.tape.setAttribute('transform', `translate(${PX + 4} ${PY + 4}) rotate(-38)`);
    const r = peelShape(p.peel, W, H).roll;
    if (r) {
      const stage = P => P.map(([x, y]) => [PX + x, PY + y]), top = stage(r.top), bot = stage(r.bot);
      el.roll.style.display = ''; el.roll.setAttribute('transform', '');
      el.rollbody.setAttribute('d', smooth([...top, ...bot.slice().reverse()]));
      el.rl1.setAttribute('d', line(mid(top, bot, .38).slice(1, -1)));
      el.rl2.setAttribute('d', line(mid(top, bot, .68).slice(2, -2)));
    } else el.roll.style.display = 'none';
    el.hand.style.opacity = '0'; el.slap.style.opacity = '0';
  } else {
    const fo = Math.min(1, p.wave / 14) * .18;
    el.folds.forEach((f, i) => {
      const k = [.28, .45, .62, .8][i];
      const a = pt(p, k, .12), m = pt(p, k + .06, .5), b = pt(p, k + .02, .86);
      f.setAttribute('d', `M${f1(a[0])} ${f1(a[1])} Q${f1(m[0])} ${f1(m[1])} ${f1(b[0])} ${f1(b[1])}`);
      f.style.opacity = String(fo);
    });
    // the curled-up end: a cylinder seen side-on, at the free end
    const R = p.roll;
    if (R > 1) {
      const [ex, ey] = pt(p, 1, .5), h = H * p.wid + 8, P = [];
      el.roll.style.display = '';
      el.roll.setAttribute('transform', `translate(${f1(ex)} ${f1(ey)}) rotate(${f1(p.angle)})`);
      for (let k = 0; k <= 6; k++) P.push([R * (1 + (noise(k, 5) - .5) * .4), -h / 2 + h * k / 6]);
      for (let k = 6; k >= 0; k--) P.push([-R * (1 + (noise(k, 6) - .5) * .4), -h / 2 + h * k / 6]);
      el.rollbody.setAttribute('d', smooth(P));
      el.rl1.setAttribute('d', ''); el.rl2.setAttribute('d', '');   // the peel's roll lines don't belong here
    } else el.roll.style.display = 'none';
    // the printing: clipped while it rolls up, bent along as it hangs
    el.print.setAttribute('transform', `translate(${PX} ${PY}) rotate(${f1(p.angle)}) scale(${p.printLen.toFixed(3)} ${(p.wid * (1 - p.taper * .5)).toFixed(3)}) translate(${-PX} ${-PY})`);
    el.tape.setAttribute('transform', `translate(${PX + 4} ${PY + 4}) rotate(${f1(-38 + p.angle * .1)})`);
    const { x, y, rot, visible } = p.hand;
    el.hand.setAttribute('transform', `translate(${f1(x - 76)} ${f1(y - 96)}) rotate(${f1(rot)} 76 96) scale(${p.slapping ? 1.06 : 1})`);
    el.hand.style.opacity = String(visible);
    el.slap.style.opacity = p.slapping ? '1' : '0';
  }
  el.neon.classList.toggle('dim', flicker && Math.random() < .07);
}

/**
 * Run the loop in the headline's SVG layer. Under reduced motion: draw the hanging scrap once, start nothing.
 * Otherwise one setTimeout chain at 1000 / FPS, paused while the headline is off screen or the tab is hidden.
 */
export function startHero(svg, { reduced = matchMedia('(prefers-reduced-motion: reduce)').matches } = {}) {
  const doc = svg.ownerDocument, $ = id => doc.getElementById('hero-' + id);
  const el = {
    cloth: $('cloth'), clip: $('clip'), print: $('print'), tape: $('tape'), hand: $('hand'), slap: $('slap'),
    neon: $('neon'), roll: $('roll'), rollbody: $('rollbody'), rl1: $('rl1'), rl2: $('rl2'),
    folds: [...$('folds').children],
  };
  if (reduced) {
    draw(el, HANG_POSE, false);
    return { stop() {} };
  }
  // the loop runs on the document's clock (0 = navigation start) until it is first paused
  const now = () => performance.now();
  let t0 = 0, heldAt = null, timer = null, onScreen = true, stopped = false;
  const running = () => !stopped && onScreen && !doc.hidden;
  function tick() {
    const t = quantize((now() - t0) / 1000, FPS);
    draw(el, pose(t, (Math.random() - .5) * 1.5), true);
    timer = setTimeout(tick, 1000 / FPS);
  }
  function update() {
    if (running() && timer === null) { if (heldAt !== null) t0 = now() - heldAt * 1000; tick(); }
    else if (!running() && timer !== null) { clearTimeout(timer); timer = null; heldAt = wrap((now() - t0) / 1000); }
  }
  const onVis = () => update();
  doc.addEventListener('visibilitychange', onVis);
  let io = null;
  if (typeof IntersectionObserver === 'function') {
    io = new IntersectionObserver(es => { onScreen = es[es.length - 1].isIntersecting; update(); });
    io.observe(svg.parentNode || svg);
  }
  update();
  return {
    stop() {
      stopped = true; update();
      doc.removeEventListener('visibilitychange', onVis);
      if (io) io.disconnect();
    },
  };
}
