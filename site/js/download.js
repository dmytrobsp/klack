// The download button: picks the visitor's system. No network of its own beyond data/release.json (same origin).
//
// MARKUP CONTRACT (Task 4 generates this region, between <!-- fill:download-button --> markers,
// inside <div class="dl-area">; every href is real, so the page works with JS off):
//   <a class="dl" data-os="linux" href="<linux zip url>">DOWNLOAD FOR LINUX</a>
//   <span class="dl-meta">v<version> · <size MB> · <span class="dl-others">
//     <a href="<macOS zip>">macOS</a> · <a href="<windows zip>">Windows</a> · <a href="#get">all downloads</a></span></span>
// enhance() replaces the whole region's innerHTML with the same shape for the visitor's system:
//   macOS: two buttons a.dl "Apple Silicon" / "Intel"; mobile/unknown: one a.dl "ALL DOWNLOADS" -> #get.
// From 0.11.0 (installs(): the first release the install scripts carry; fill.py's render_hero_install writes the
// Linux one) the visitor's one-liner leads instead, its zip beside it:
//   <code class="dl-cmd" data-os="linux"><span class="ps">$</span> curl -fsSL https://ttyak.sh/install.sh | sh</code>
//   <a class="dl small" data-os="linux" href="<zip>">DOWNLOAD</a>
//   <span class="dl-meta">v<version> · zip <size MB> · <span class="dl-others"><a href="#get">other systems and
//     ways to install</a></span></span>
//   Windows: `> irm https://ttyak.sh/install.ps1 | iex`; macOS: the Apple Silicon zip, "Intel zip" in the meta line;
//   mobile/unknown: one a.dl "HOW TO INSTALL" -> #get.

const TABLE = {
  linux: { label: 'DOWNLOAD FOR LINUX', picks: [['x86_64-linux', null]] },
  windows: { label: 'DOWNLOAD FOR WINDOWS', picks: [['x86_64-windows', null]] },
  macos: { label: 'DOWNLOAD FOR MACOS', picks: [['aarch64-macos', 'Apple Silicon'], ['x86_64-macos', 'Intel']] },
};

// the one-liner per system: its prompt and the command
const ONE_LINER = {
  linux: ['$', 'curl -fsSL https://ttyak.sh/install.sh | sh'],
  macos: ['$', 'curl -fsSL https://ttyak.sh/install.sh | sh'],
  windows: ['&gt;', 'irm https://ttyak.sh/install.ps1 | iex'],
};
const ZIP = { linux: 'x86_64-linux', macos: 'aarch64-macos', windows: 'x86_64-windows' };

/** Whether the install scripts carry this release: a release version, 0.11.0 or later. */
export function installs(version) {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(String(version ?? ''));
  if (!m) return false;
  const [a, b] = [Number(m[1]), Number(m[2])];
  return a > 0 || b >= 11;
}

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

// The hero from 0.11.0: the visitor's one-liner, its zip beside it, a way to the other systems and ways.
function installArea(os, release) {
  const ver = release.version ? `v${esc(release.version)} · ` : '';
  const one = ONE_LINER[os];
  if (!one) return `<a class="dl" data-os="${os}" href="#get">HOW TO INSTALL</a><span class="dl-meta">${ver}macOS · Windows · Linux</span>`;
  const zip = release.assets.find(a => a.platform === ZIP[os]);
  const intel = os === 'macos' && release.assets.find(a => a.platform === 'x86_64-macos');
  const button = zip ? `<a class="dl small" data-os="${os}" href="${esc(zip.url)}">DOWNLOAD</a>` : '';
  const size = zip ? `${os === 'macos' ? 'Apple Silicon zip' : 'zip'} ${mb(zip.size)} · ` : '';
  const others = (intel ? `<a href="${esc(intel.url)}">Intel zip</a> · ` : '') + '<a href="#get">other systems and ways to install</a>';
  return `<code class="dl-cmd" data-os="${os}"><span class="ps">${one[0]}</span> ${esc(one[1])}</code>${button}`
    + `<span class="dl-meta">${ver}${size}<span class="dl-others">${others}</span></span>`;
}

export function enhance(doc, release, nav = typeof navigator !== 'undefined' ? navigator : {}) {
  const area = doc && doc.querySelector('.dl-area');
  if (!area || !release || !release.assets) return;
  const os = detectOS(nav);
  if (installs(release.version)) {
    area.innerHTML = installArea(os, release);
    return;
  }
  const c = choose(os, release);
  const ver = release.version ? `v${esc(release.version)} · ` : '';
  const find = p => release.assets.find(a => a.platform === p);
  let buttons, meta;
  if (!c.assets.length) {
    buttons = `<a class="dl" data-os="${os}" href="#get">${c.label}</a>`;
    meta = `${ver}macOS · Windows · Linux`;
  } else {
    buttons = c.assets.map(a => `<a class="dl" data-os="${os}" href="${esc(a.url)}">${esc(a.label)}</a>`).join('');
    const sizes = c.assets.map(a => mb(a.size)).join(' / ');
    const others = [['linux', 'Linux', 'x86_64-linux'], ['macos', 'macOS', 'aarch64-macos'], ['windows', 'Windows', 'x86_64-windows']]
      .filter(([k]) => k !== os)
      .map(([, name, p]) => (find(p) ? `<a href="${esc(find(p).url)}">${name}</a>` : name));
    meta = `${ver}${sizes} · <span class="dl-others">${others.join(' · ')} · <a href="#get">all downloads</a></span>`;
  }
  area.innerHTML = `${buttons}<span class="dl-meta">${meta}</span>`;
}

export async function load(doc = document) {
  try {
    const r = await fetch('data/release.json');
    if (r.ok) enhance(doc, await r.json());
  } catch (e) { /* the static (Linux) button stays */ }
}
