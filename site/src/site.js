// Kantor download site: pick the right download, show the latest version,
// screenshot tabs and the copy button. Everything works without this script;
// it only fills in details.
(() => {
  "use strict";
  const t = JSON.parse(document.getElementById("i18n").textContent);
  const $ = (sel, el = document) => el.querySelector(sel);
  const $$ = (sel, el = document) => Array.from(el.querySelectorAll(sel));

  // ---------- Which computer is this? ----------
  async function detect() {
    const ua = navigator.userAgent || "";
    const data = navigator.userAgentData;
    const platform = (data && data.platform) || "";
    const touchMac = /Macintosh/.test(ua) && navigator.maxTouchPoints > 1; // iPadOS
    const mobile = (data && data.mobile) || /Android|iPhone|iPad|iPod|Mobile/.test(ua) || touchMac;
    if (mobile) return { os: "mobile" };
    if (/mac/i.test(platform) || /Macintosh|Mac OS X/.test(ua)) return { os: "mac" };
    if (/win/i.test(platform) || /Windows/.test(ua)) {
      let arm = /ARM64|aarch64/i.test(ua);
      if (data && data.getHighEntropyValues) {
        try {
          const v = await data.getHighEntropyValues(["architecture"]);
          if (v.architecture) arm = v.architecture === "arm";
        } catch (_) { /* keep the guess from the user agent */ }
      }
      return { os: "windows", arm };
    }
    if (/CrOS/.test(ua) || /chrome os/i.test(platform)) return { os: "other" };
    if (/linux/i.test(platform) || /Linux|X11/.test(ua)) {
      let arm = /aarch64|arm64|armv8/i.test(ua);
      if (data && data.getHighEntropyValues) {
        try {
          const v = await data.getHighEntropyValues(["architecture"]);
          if (v.architecture) arm = v.architecture === "arm";
        } catch (_) { /* keep the guess from the user agent */ }
      }
      return { os: "linux", arm };
    }
    return { os: "other" };
  }

  function setCta(info) {
    const main = $("#cta-main");
    const label = $(".btn-label", main);
    const meta = $("#cta-meta");
    const other = $("#cta-other");
    if (info.os === "mac") {
      main.href = `${t.dl}/Kantor.dmg`;
      main.dataset.asset = "Kantor.dmg";
      label.textContent = t.ctaMac;
      meta.textContent = t.ctaMacMeta;
      other.hidden = false;
    } else if (info.os === "linux") {
      const file = info.arm ? "Kantor-aarch64.AppImage" : "Kantor-x86_64.AppImage";
      main.href = `${t.dl}/${file}`;
      main.dataset.asset = file;
      label.textContent = t.ctaLinux;
      meta.textContent = info.arm ? t.ctaLinuxMetaArm : t.ctaLinuxMetaX64;
      other.hidden = false;
    } else if (info.os === "windows" && t.windows) {
      const file = info.arm ? "Kantor-Setup-arm64.exe" : "Kantor-Setup-x64.exe";
      main.href = `${t.dl}/${file}`;
      main.dataset.asset = file;
      label.textContent = t.ctaWin;
      meta.textContent = info.arm ? t.ctaWinMetaArm : t.ctaWinMeta;
      other.hidden = false;
    } else if (info.os === "windows") {
      label.textContent = t.ctaWindows;
      meta.textContent = t.ctaWindowsNote;
    } else if (info.os === "mobile") {
      label.textContent = t.ctaMobile;
      meta.textContent = t.ctaMobileNote;
    }
  }

  // Put the visitor's platform first in the DOM, so visual and tab order match.
  function markPlatform(info) {
    const box = $("#platforms");
    const card = $(`[data-os="${info.os}"]`, box);
    if (!card) return;
    card.classList.add("is-yours");
    $(".yours", card).hidden = false;
    box.prepend(card);
    if (info.os === "linux" && info.arm) {
      // ARM Linux: make the aarch64 file the primary button.
      const x64 = $("#linux-x64");
      const arm = $("#linux-arm");
      x64.classList.replace("btn-primary", "btn-secondary");
      arm.classList.replace("btn-secondary", "btn-primary");
      arm.parentElement.prepend(arm);
    }
    if (info.os === "windows" && info.arm && $("#win-arm")) {
      const x64 = $("#win-x64");
      const arm = $("#win-arm");
      x64.classList.replace("btn-primary", "btn-secondary");
      arm.classList.replace("btn-secondary", "btn-primary");
      arm.parentElement.prepend(arm);
    }
  }

  // Files the latest release doesn't have yet; the hero button falls back to the list.
  const missing = new Set();
  function heroFallback() {
    const main = $("#cta-main");
    if (main.dataset.asset && missing.has(main.dataset.asset)) main.href = "#download";
  }

  // ---------- Latest release ----------
  const API = "https://api.github.com/repos/dzulfikar08/kantor/releases/latest";
  const CACHE = "kantor-latest-release";

  function fill(str, vars) {
    return str.replace(/\{(\w+)\}/g, (_, k) => (k in vars ? vars[k] : ""));
  }
  function escapeHtml(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  }

  async function latestRelease() {
    try {
      const hit = JSON.parse(sessionStorage.getItem(CACHE) || "null");
      if (hit && Date.now() - hit.at < 10 * 60 * 1000) return hit.value;
    } catch (_) { /* storage may be blocked */ }
    const ctrl = new AbortController();
    const timer = setTimeout(() => ctrl.abort(), 8000);
    let value;
    try {
      const res = await fetch(API, { headers: { Accept: "application/vnd.github+json" }, signal: ctrl.signal });
      if (res.status === 404) value = { none: true };
      else if (!res.ok) throw new Error(String(res.status));
      else {
        const r = await res.json();
        value = {
          tag: r.tag_name,
          date: r.published_at,
          url: r.html_url,
          assets: (r.assets || []).map((a) => a.name),
        };
      }
    } finally {
      clearTimeout(timer);
    }
    try { sessionStorage.setItem(CACHE, JSON.stringify({ at: Date.now(), value })); } catch (_) { /* ignore */ }
    return value;
  }

  async function showVersion() {
    const el = $("#version");
    el.textContent = t.verLoading;
    el.setAttribute("aria-busy", "true");
    try {
      const r = await latestRelease();
      if (r.none) {
        el.textContent = t.verNone;
        return;
      }
      const date = r.date
        ? new Intl.DateTimeFormat(t.lang === "id" ? "id-ID" : "en-GB", { day: "numeric", month: "long", year: "numeric" }).format(new Date(r.date))
        : "";
      el.textContent = fill(t.verOk, { tag: r.tag, date }) + " ";
      const a = document.createElement("a");
      a.href = r.url;
      a.textContent = t.verNotes;
      el.append(a);
      // If a file isn't in this release yet, say so next to its button instead of
      // letting the button lead to a GitHub error page.
      for (const note of $$(".asset-note")) {
        const absent = note.dataset.for.split(" ").filter((f) => r.assets.length && !r.assets.includes(f));
        if (absent.length) {
          note.innerHTML = fill(t.assetMissing, { tag: escapeHtml(r.tag), url: escapeHtml(r.url) });
          note.hidden = false;
          absent.forEach((f) => missing.add(f));
        }
      }
      heroFallback();
    } catch (_) {
      el.textContent = t.verError;
    } finally {
      el.removeAttribute("aria-busy");
    }
  }

  // ---------- Screenshot tabs (WAI-ARIA tabs, automatic activation) ----------
  function initTabs() {
    const tabs = $$('[role="tab"]');
    const select = (tab, focus) => {
      for (const other of tabs) {
        const on = other === tab;
        other.setAttribute("aria-selected", String(on));
        other.tabIndex = on ? 0 : -1;
        document.getElementById(other.getAttribute("aria-controls")).hidden = !on;
      }
      if (focus) tab.focus();
    };
    tabs.forEach((tab, i) => {
      tab.addEventListener("click", () => select(tab, false));
      tab.addEventListener("keydown", (e) => {
        let next = null;
        if (e.key === "ArrowRight") next = tabs[(i + 1) % tabs.length];
        else if (e.key === "ArrowLeft") next = tabs[(i - 1 + tabs.length) % tabs.length];
        else if (e.key === "Home") next = tabs[0];
        else if (e.key === "End") next = tabs[tabs.length - 1];
        if (next) {
          e.preventDefault();
          select(next, true);
        }
      });
    });
  }

  // ---------- Copy the install command ----------
  function initCopy() {
    for (const btn of $$("[data-copy]")) {
      const status = $(".copy-status", btn.closest(".term"));
      if (!navigator.clipboard) continue; // stays hidden; the text can still be selected
      btn.hidden = false;
      btn.addEventListener("click", async () => {
        const text = document.getElementById(btn.dataset.copy).textContent.trim();
        try {
          await navigator.clipboard.writeText(text);
          btn.textContent = t.copied;
          status.textContent = "";
          setTimeout(() => { btn.textContent = t.copy; }, 2000);
        } catch (_) {
          status.textContent = t.copyFail;
        }
      });
    }
  }

  initTabs();
  initCopy();
  showVersion();
  detect().then((info) => {
    setCta(info);
    markPlatform(info);
    heroFallback();
  });
})();
