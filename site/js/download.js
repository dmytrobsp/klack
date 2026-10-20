// The download button: picks the visitor's system. No network of its own beyond data/release.json (same origin).
//
// MARKUP CONTRACT (Task 4 generates this region, between <!-- fill:download-button --> markers,
// inside <div class="dl-area">; every href is real, so the page works with JS off):
//   <a class="dl" data-os="linux" href="<linux zip url>">DOWNLOAD FOR LINUX</a>
//   <span class="dl-meta">v<version> · <size MB> · <span class="dl-others">
//     <a href="<macOS zip>">macOS</a> · <a href="<windows zip>">Windows</a> · <a href="#get">all downloads</a></span></span>
// enhance() replaces the whole region's innerHTML with the same shape for the visitor's system:
//   macOS: two buttons a.dl "Apple Silicon" / "Intel"; mobile/unknown: one a.dl "ALL DOWNLOADS" -> #get.

const TABLE = {
  linux: { label: 'DOWNLOAD FOR LINUX', picks: [['x86_64-linux', null]] },
  windows: { label: 'DOWNLOAD FOR WINDOWS', picks: [['x86_64-windows', null]] },
  macos: { label: 'DOWNLOAD FOR MACOS', picks: [['aarch64-macos', 'Apple Silicon'], ['x86_64-macos', 'Intel']] },
};

export function detectOS({ userAgent = '', platform = '', maxTouchPoints = 0 } = {}) {
  const ua = userAgent || '';
  if (/Android|iPhone|iPad|iPod/i.test(ua)) return 'mobile';
  if (platform === 'MacIntel' && maxTouchPoints > 1) return 'mobile'; // iPadOS poses as a Mac
  if (/Windows/i.test(ua)) return 'windows';
  if (/Macintosh|Mac OS X/i.test(ua)) return 'macos';
  if (/CrOS/i.test(ua)) return 'unknown';
  if (/Linux|X11/i.test(ua)) return 'linux';
  return 'unknown';
}

export function choose(os, release) {
  const row = TABLE[os];
  if (!row) return { label: 'ALL DOWNLOADS', href: '#get', assets: [] };
  const assets = [];
  for (const [platform, label] of row.picks) {
    const a = (release.assets || []).find(x => x.platform === platform);
    if (a) assets.push({ ...a, label: label || row.label });
  }
  if (!assets.length) return { label: 'ALL DOWNLOADS', href: '#get', assets: [] };
  return { label: row.label, href: assets[0].url, assets };
}

const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
export const mb = n => (n / 1e6).toFixed(1) + ' MB';

export function enhance(doc, release, nav = typeof navigator !== 'undefined' ? navigator : {}) {
  const area = doc && doc.querySelector('.dl-area');
  if (!area || !release || !release.assets) return;
  const os = detectOS(nav);
  const c = choose(os, release);
  const find = p => release.assets.find(a => a.platform === p);
  let buttons, meta;
  if (!c.assets.length) {
    buttons = `<a class="dl" data-os="${os}" href="#get">${c.label}</a>`;
    meta = `v${esc(release.version)} · macOS · Windows · Linux`;
  } else {
    buttons = c.assets.map(a => `<a class="dl" data-os="${os}" href="${esc(a.url)}">${esc(a.label)}</a>`).join('');
    const sizes = c.assets.map(a => mb(a.size)).join(' / ');
    const others = [['linux', 'Linux', 'x86_64-linux'], ['macos', 'macOS', 'aarch64-macos'], ['windows', 'Windows', 'x86_64-windows']]
      .filter(([k]) => k !== os)
      .map(([, name, p]) => (find(p) ? `<a href="${esc(find(p).url)}">${name}</a>` : name));
    meta = `v${esc(release.version)} · ${sizes} · <span class="dl-others">${others.join(' · ')} · <a href="#get">all downloads</a></span>`;
  }
  area.innerHTML = `${buttons}<span class="dl-meta">${meta}</span>`;
}

export async function load(doc = document) {
  try {
    const r = await fetch('data/release.json');
    if (r.ok) enhance(doc, await r.json());
  } catch (e) { /* the static (Linux) button stays */ }
}
