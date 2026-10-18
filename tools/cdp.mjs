// Minimal Chrome DevTools Protocol driver for a test browser on localhost.
// usage: node cdp.mjs PORT cookie SESSION_JSON   (sets Slack's `d` cookie; never prints it)
//        node cdp.mjs PORT goto URL
//        node cdp.mjs PORT eval JS                (prints the JSON result)
//        node cdp.mjs PORT shot FILE.png
import { readFileSync, writeFileSync } from 'node:fs';

const [port, cmd, arg] = process.argv.slice(2);
const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = targets.find((t) => t.type === 'page');
if (!page) throw new Error('no page target');
const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((ok, fail) => { ws.onopen = ok; ws.onerror = fail; });
let id = 0;
const pending = new Map();
ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id && pending.has(msg.id)) {
    const { ok, fail } = pending.get(msg.id);
    pending.delete(msg.id);
    msg.error ? fail(new Error(msg.error.message)) : ok(msg.result);
  }
};
const send = (method, params = {}) => new Promise((ok, fail) => {
  const n = ++id;
  pending.set(n, { ok, fail });
  ws.send(JSON.stringify({ id: n, method, params }));
});

if (cmd === 'cookie') {
  const s = JSON.parse(readFileSync(arg, 'utf8'));
  const value = s.workspaces[0].cookie_d;
  const r = await send('Network.setCookie', { name: 'd', value, domain: '.slack.com', path: '/', secure: true, httpOnly: true, sameSite: 'Lax' });
  console.log('cookie set:', r.success);
} else if (cmd === 'goto') {
  await send('Page.navigate', { url: arg });
  console.log('navigating');
} else if (cmd === 'eval') {
  const r = await send('Runtime.evaluate', { expression: arg, awaitPromise: true, returnByValue: true });
  console.log(JSON.stringify(r.result.value ?? r.exceptionDetails?.text ?? null));
} else if (cmd === 'click') {
  // A real mouse click at the centre of the first element matching the selector.
  const r = await send('Runtime.evaluate', { expression: `(() => { const e = document.querySelector(${JSON.stringify(arg)}); if (!e) return null; const b = e.getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`, returnByValue: true });
  const at = r.result.value;
  if (!at) { console.log('not found:', arg); } else {
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await send('Input.dispatchMouseEvent', { type, x: at[0], y: at[1], button: 'left', clickCount: 1 });
    }
    console.log('clicked', arg, at.map(Math.round));
  }
} else if (cmd === 'shot') {
  const r = await send('Page.captureScreenshot', { format: 'png' });
  writeFileSync(arg, Buffer.from(r.data, 'base64'));
  console.log(arg);
}
ws.close();
