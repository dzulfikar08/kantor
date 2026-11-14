// Builds the static site into site/dist. No dependencies: `node site/build.mjs`.
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { en, id } from "./src/strings.mjs";

const root = dirname(fileURLToPath(import.meta.url));
const src = join(root, "src");
const dist = join(root, "dist");
const SITE = "https://kantor.paparanai.id";
const GH = "https://github.com/dzulfikar08/kantor";
const DL = `${GH}/releases/latest/download`;
// Flip to true for the first release that has Kantor-Setup-{x64,arm64}.exe and
// Kantor-{x64,arm64}.zip (release.yml), then rebuild and redeploy. Until then the Windows card
// says "not available yet" and Windows visitors are pointed at the other downloads.
// KANTOR_SITE_WINDOWS=1 node site/build.mjs previews the Windows downloads.
const WINDOWS_RELEASED = true; // v0.4.0 (2026-11-13) is the first release with Windows downloads
const SHOT_W = 1550;
const SHOT_H = 975;

rmSync(dist, { recursive: true, force: true });
mkdirSync(join(dist, "assets"), { recursive: true });
mkdirSync(join(dist, "id"), { recursive: true });

// Fingerprinted CSS and JS, so they can be cached for a year.
function asset(name) {
  const body = readFileSync(join(src, name));
  const hash = createHash("sha256").update(body).digest("hex").slice(0, 10);
  const [base, ext] = name.split(".");
  const out = `${base}.${hash}.${ext}`;
  writeFileSync(join(dist, "assets", out), body);
  return `/assets/${out}`;
}
const css = asset("site.css");
const js = asset("site.js");

cpSync(join(src, "img"), join(dist, "img"), { recursive: true });
// Images keep their names; a content hash in the query makes a new screenshot show at once
// (the CDN and browsers cache /img/* for a day).
const img = (name) =>
  `/img/${name}?v=${createHash("sha256").update(readFileSync(join(src, "img", name))).digest("hex").slice(0, 10)}`;
for (const f of readdirSync(join(root, "static"))) cpSync(join(root, "static", f), join(dist, f));

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const APPS = ["sheets", "docs", "slides"];

function picture(app, t, first) {
  const set = (theme, ext) => `${img(`${app}-${theme}-800.${ext}`)} 800w, ${img(`${app}-${theme}-1550.${ext}`)} 1550w`;
  const sizes = "(min-width: 1180px) 1100px, calc(100vw - 2.5rem)";
  const sources = ["dark", "light"]
    .flatMap((theme) =>
      ["avif", "webp", "png"].map(
        (ext) =>
          `<source type="image/${ext}" media="(prefers-color-scheme: ${theme})" srcset="${set(theme, ext)}" sizes="${sizes}">`,
      ),
    )
    .join("\n          ");
  const load = first ? 'fetchpriority="high"' : 'loading="lazy"';
  return `<picture>
          ${sources}
          <img src="${img(`${app}-light-1550.png`)}" srcset="${set("light", "png")}" sizes="${sizes}" width="${SHOT_W}" height="${SHOT_H}" alt="${esc(t.shots[app].alt)}" ${load} decoding="async">
        </picture>`;
}

// The page text with Windows downloads switched on.
function withWindows(t) {
  return {
    ...t,
    title: t.win.title,
    lede: t.win.lede,
    facts: [...t.facts.slice(0, -1), t.win.fact],
    ctaMobileNote: t.win.ctaMobileNote,
  };
}

function windowsCard(t) {
  if (!WINDOWS_RELEASED) {
    return `<article class="platform platform-off" id="p-windows" data-os="windows" aria-labelledby="win-h">
            <p class="yours" hidden>${t.yourComputer}</p>
            <h3 id="win-h">${t.windows.name}</h3>
            <p>${t.windows.text}</p>
          </article>`;
  }
  const w = t.win;
  return `<article class="platform" id="p-windows" data-os="windows" aria-labelledby="win-h">
            <p class="yours" hidden>${t.yourComputer}</p>
            <h3 id="win-h">${t.windows.name}</h3>
            <p class="req">${w.req}</p>
            <div class="btn-row">
              <a class="btn btn-primary" id="win-x64" href="${DL}/Kantor-Setup-x64.exe" data-asset="Kantor-Setup-x64.exe"><span class="btn-label">${w.buttonX64}</span><span class="btn-meta">${w.buttonX64Meta}</span></a>
              <a class="btn btn-secondary" id="win-arm" href="${DL}/Kantor-Setup-arm64.exe" data-asset="Kantor-Setup-arm64.exe"><span class="btn-label">${w.buttonArm}</span><span class="btn-meta">${w.buttonArmMeta}</span></a>
            </div>
            <p class="asset-note" data-for="Kantor-Setup-x64.exe Kantor-Setup-arm64.exe" hidden></p>
            <ol class="steps">${w.steps.map((s) => `<li>${s}</li>`).join("")}</ol>
            <p class="req">${w.portable.replaceAll("{dl}", DL)}</p>
          </article>`;
}

function page(t, other) {
  if (WINDOWS_RELEASED) t = withWindows(t);
  const url = SITE + t.path;
  const ogImage = `${SITE}${img("og.png")}`;
  // Text the script needs for the states it switches between.
  const i18n = {
    lang: t.lang,
    ctaChoose: t.ctaChoose,
    ctaMac: t.ctaMac,
    ctaMacMeta: t.ctaMacMeta,
    ctaLinux: t.ctaLinux,
    ctaLinuxMetaX64: t.ctaLinuxMetaX64,
    ctaLinuxMetaArm: t.ctaLinuxMetaArm,
    ctaWindows: t.ctaWindows,
    ctaWindowsNote: t.ctaWindowsNote,
    windows: WINDOWS_RELEASED,
    ctaWin: t.win.cta,
    ctaWinMeta: t.win.ctaMeta,
    ctaWinMetaArm: t.win.ctaMetaArm,
    ctaMobile: t.ctaMobile,
    ctaMobileNote: t.ctaMobileNote,
    verLoading: t.verLoading,
    verOk: t.verOk,
    verNotes: t.verNotes,
    verError: t.verError,
    verNone: t.verNone,
    assetMissing: t.assetMissing,
    copied: t.linux.copied,
    copy: t.linux.copy,
    copyFail: t.linux.copyFail,
    dl: DL,
  };

  return `<!doctype html>
<html lang="${t.lang}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${esc(t.title)}</title>
  <meta name="description" content="${esc(t.description)}">
  <link rel="canonical" href="${url}">
  <link rel="alternate" hreflang="en" href="${SITE}/">
  <link rel="alternate" hreflang="id" href="${SITE}/id/">
  <link rel="alternate" hreflang="x-default" href="${SITE}/">
  <meta name="color-scheme" content="light dark">
  <meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f7fa">
  <meta name="theme-color" media="(prefers-color-scheme: dark)" content="#11141c">
  <link rel="icon" href="/favicon.ico" sizes="48x48">
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="apple-touch-icon" href="/apple-touch-icon.png">
  <meta property="og:type" content="website">
  <meta property="og:site_name" content="Kantor">
  <meta property="og:title" content="${esc(t.title)}">
  <meta property="og:description" content="${esc(t.description)}">
  <meta property="og:url" content="${url}">
  <meta property="og:locale" content="${t.locale}">
  <meta property="og:locale:alternate" content="${other.locale}">
  <meta property="og:image" content="${ogImage}">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="${esc(t.ogAlt)}">
  <meta name="twitter:card" content="summary_large_image">
  <meta name="twitter:title" content="${esc(t.title)}">
  <meta name="twitter:description" content="${esc(t.description)}">
  <meta name="twitter:image" content="${ogImage}">
  <meta name="twitter:image:alt" content="${esc(t.ogAlt)}">
  <link rel="stylesheet" href="${css}">
  <script src="${js}" defer></script>
  <script type="application/json" id="i18n">${JSON.stringify(i18n).replace(/</g, "\\u003c")}</script>
</head>
<body>
  <a class="skip" href="#main">${t.skip}</a>
  <header class="top">
    <div class="wrap top-row">
      <a class="brand" href="${t.path}"><img src="/favicon.svg" width="28" height="28" alt="">Kantor</a>
      <nav aria-label="${t.navLabel}">
        <ul class="nav">
          <li><a href="#download">${t.navDownload}</a></li>
          <li class="nav-limits"><a href="#limits">${t.navLimits}</a></li>
          <li class="nav-gh"><a href="${GH}">${t.navSource}</a></li>
          <li><a class="lang" href="${other.path}" hreflang="${other.lang}" lang="${other.lang}" aria-label="${t.switchLangLabel}"><span class="lang-long">${t.switchLang}</span><span class="lang-short" aria-hidden="true">${t.switchLangShort}</span></a></li>
        </ul>
      </nav>
    </div>
  </header>

  <main id="main">
    <section class="hero wrap" aria-labelledby="h1">
      <div class="hero-text">
        <h1 id="h1">${t.h1}</h1>
        <p class="lede">${t.lede}</p>
        <div class="cta" id="cta">
          <a class="btn btn-primary" id="cta-main" href="#download"><span class="btn-label">${t.ctaChoose}</span></a>
          <a class="cta-other" id="cta-other" href="#download" hidden>${t.ctaOtherFiles}</a>
          <p class="cta-meta" id="cta-meta"></p>
        </div>
      </div>
      <div class="hero-aside">
        <ul class="facts">${t.facts.map((f) => `<li>${f}</li>`).join("")}</ul>
        <p class="preview">${t.preview}</p>
      </div>

      <figure class="shots">
        <div class="tabs" role="tablist" aria-label="${t.shotsLabel}">
          ${APPS.map(
            (a, i) =>
              `<button class="tab tab-${a}" role="tab" id="tab-${a}" aria-controls="panel-${a}" aria-selected="${i === 0}" tabindex="${i === 0 ? 0 : -1}" type="button">${t.shots[a].tab}</button>`,
          ).join("\n          ")}
        </div>
        ${APPS.map(
          (a, i) => `<div class="panel" role="tabpanel" tabindex="0" id="panel-${a}" aria-labelledby="tab-${a}"${i === 0 ? "" : " hidden"}>
        <div class="window">
        ${picture(a, t, i === 0)}
        </div>
        <p class="caption">${t.shots[a].caption}</p>
      </div>`,
        ).join("\n        ")}
        <figcaption class="shots-note">${t.shotsNote}</figcaption>
      </figure>
    </section>

    <section class="what" aria-labelledby="what-h">
      <div class="wrap">
        <h2 id="what-h">${t.whatH2}</h2>
        <ul class="apps">
          ${APPS.map(
            (a) => `<li class="app app-${a}">
            <h3>${t.apps[a].name}</h3>
            <p class="formats">${t.apps[a].formats.map((f) => `<span class="kbd">${f}</span>`).join(" ")}</p>
            <p>${t.apps[a].text}</p>
          </li>`,
          ).join("\n          ")}
        </ul>
        <div class="keep">
          <h3>${t.keepH3}</h3>
          <p>${t.keepText}</p>
        </div>
        <dl class="more">
          ${t.more.map(([h, p]) => `<div><dt>${h}</dt><dd>${p}</dd></div>`).join("\n          ")}
        </dl>
      </div>
    </section>

    <section class="download" id="download" aria-labelledby="dl-h" tabindex="-1">
      <div class="wrap">
        <div class="dl-head">
          <h2 id="dl-h">${t.dlH2}</h2>
          <p>${t.dlIntro}</p>
          <p class="version" id="version" role="status" aria-live="polite"></p>
        </div>
        <div class="platforms" id="platforms">
          <article class="platform" id="p-mac" data-os="mac" aria-labelledby="mac-h">
            <p class="yours" hidden>${t.yourComputer}</p>
            <h3 id="mac-h">${t.mac.name}</h3>
            <p class="req">${t.mac.req}</p>
            <a class="btn btn-primary" href="${DL}/Kantor.dmg" data-asset="Kantor.dmg"><span class="btn-label">${t.mac.button}</span></a>
            <p class="asset-note" data-for="Kantor.dmg" hidden></p>
            <ol class="steps">${t.mac.steps.map((s) => `<li>${s}</li>`).join("")}</ol>
          </article>
          <article class="platform" id="p-linux" data-os="linux" aria-labelledby="linux-h">
            <p class="yours" hidden>${t.yourComputer}</p>
            <h3 id="linux-h">${t.linux.name}</h3>
            <p class="req">${t.linux.req}</p>
            <div class="btn-row">
              <a class="btn btn-primary" id="linux-x64" href="${DL}/Kantor-x86_64.AppImage" data-asset="Kantor-x86_64.AppImage"><span class="btn-label">${t.linux.buttonX64}</span><span class="btn-meta">${t.linux.buttonX64Meta}</span></a>
              <a class="btn btn-secondary" id="linux-arm" href="${DL}/Kantor-aarch64.AppImage" data-asset="Kantor-aarch64.AppImage"><span class="btn-label">${t.linux.buttonArm}</span><span class="btn-meta">${t.linux.buttonArmMeta}</span></a>
            </div>
            <p class="asset-note" data-for="Kantor-x86_64.AppImage Kantor-aarch64.AppImage" hidden></p>
            <ol class="steps">${t.linux.steps.map((s) => `<li>${s}</li>`).join("")}</ol>
            <div class="term">
              <h4>${t.linux.termH}</h4>
              <p>${t.linux.termText}</p>
              <div class="cmd">
                <code id="install-cmd">curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh</code>
                <button class="copy" type="button" data-copy="install-cmd" hidden>${t.linux.copy}</button>
              </div>
              <p class="copy-status" role="status" aria-live="polite"></p>
            </div>
          </article>
          ${windowsCard(t)}
        </div>
      </div>
    </section>

    <section class="limits" id="limits" aria-labelledby="limits-h" tabindex="-1">
      <div class="wrap limits-grid">
        <div>
          <h2 id="limits-h">${t.limitsH2}</h2>
          <p>${t.limitsIntro}</p>
        </div>
        <ul class="gaps">
          ${t.limits.map((l) => `<li>${l}</li>`).join("\n          ")}
        </ul>
        <div class="report">
          <h3>${t.reportH3}</h3>
          <p>${t.reportText}</p>
          <a class="btn btn-secondary" href="${GH}/issues/new"><span class="btn-label">${t.reportButton}</span></a>
        </div>
      </div>
    </section>
  </main>

  <footer class="foot">
    <div class="wrap foot-row">
      <p class="foot-brand"><img src="/favicon.svg" width="20" height="20" alt="">Kantor</p>
      <ul class="foot-links">
        <li><a href="${GH}">${t.footSource}</a></li>
        <li><a href="${GH}/blob/main/CHANGELOG.md">${t.footChangelog}</a></li>
        <li><a href="${GH}/blob/main/LICENSE">${t.footLicense}</a></li>
        <li><a href="${other.path}" hreflang="${other.lang}" lang="${other.lang}">${t.switchLang}</a></li>
      </ul>
      <p class="by">${t.footBy} <a href="https://paparanai.id">Paparan AI</a></p>
    </div>
  </footer>
</body>
</html>
`;
}

writeFileSync(join(dist, "index.html"), page(en, id));
writeFileSync(join(dist, "id", "index.html"), page(id, en));

// Cloudflare Pages serves 404.html for unknown paths (without it, it would serve the home page).
writeFileSync(
  join(dist, "404.html"),
  `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="robots" content="noindex">
  <title>Page not found: Kantor</title>
  <link rel="icon" href="/favicon.svg" type="image/svg+xml">
  <link rel="stylesheet" href="${css}">
</head>
<body>
  <main id="main" class="wrap notfound">
    <p class="foot-brand"><img src="/favicon.svg" width="28" height="28" alt="">Kantor</p>
    <h1>This page doesn't exist</h1>
    <p lang="id">Halaman ini tidak ada.</p>
    <p><a href="/">Go to the Kantor home page</a> · <a href="/id/" lang="id">Ke halaman utama Kantor</a></p>
  </main>
</body>
</html>
`,
);

// Every key in `en` must have an Indonesian counterpart.
const missing = Object.keys(en).filter((k) => !(k in id));
if (missing.length) throw new Error(`id is missing: ${missing.join(", ")}`);
// Copy rule: no em dashes anywhere in the output.
for (const f of ["index.html", "id/index.html"]) {
  if (readFileSync(join(dist, f), "utf8").includes("\u2014")) throw new Error(`em dash in ${f}`);
}
console.log(`built ${dist}`);
