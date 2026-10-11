// Quickshort landing — nextjs.org direction: minimal motion, hairline
// cell grids, Geist. CSS-transition reveals via IntersectionObserver,
// masked headline reveal, OS-aware download, copy buttons.
// ?static renders flat for screenshots/tests; .no-js shows everything.

document.documentElement.classList.replace("no-js", "js");

const isStatic = new URLSearchParams(location.search).has("static");
const reduced =
  window.matchMedia("(prefers-reduced-motion: reduce)").matches || isStatic;
if (isStatic) {
  document.documentElement.style.scrollBehavior = "auto";
  const hero = document.querySelector(".hero");
  if (hero) hero.style.minHeight = "auto";
}

/* ---------- language: ?lang= > localStorage > en (default) ---------- */
(() => {
  const param = new URLSearchParams(location.search).get("lang");
  const saved = localStorage.getItem("qs-lang");
  QS_APPLY_LANG(param || saved || "en");
  document.querySelectorAll(".lang-sw").forEach((btn) => {
    btn.addEventListener("click", () => {
      localStorage.setItem("qs-lang", btn.dataset.lang);
      QS_APPLY_LANG(btn.dataset.lang);
    });
  });
})();

/* ---------- masked headline: slide words up on load ---------- */
requestAnimationFrame(() => {
  requestAnimationFrame(() => document.documentElement.classList.add("ready"));
});

/* ---------- scroll reveals: IntersectionObserver + CSS transition ---------- */
if (!reduced && "IntersectionObserver" in window) {
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((e, i) => {
        if (!e.isIntersecting) return;
        const el = e.target;
        el.style.transitionDelay = `${(i % 4) * 60}ms`;
        el.classList.add("shown");
        setTimeout(() => (el.style.transitionDelay = ""), 1100);
        io.unobserve(el);
      });
    },
    { rootMargin: "0px 0px -10% 0px" }
  );
  document.querySelectorAll(".reveal").forEach((el) => io.observe(el));
} else {
  document.querySelectorAll(".reveal").forEach((el) => el.classList.add("shown"));
  document.querySelectorAll(".headline .line > span").forEach((el) => {
    el.style.transform = "none";
  });
}

/* ---------- OS detection: pick the right download ---------- */
(() => {
  const ua = navigator.userAgent;
  const plat = navigator.userAgentData?.platform || navigator.platform || "";
  let os = "macos";
  if (/Windows/i.test(ua + plat)) os = "windows";
  else if (/Android|Linux/i.test(ua + plat) && !/Mac/i.test(plat)) os = "linux";
  else if (/Mac|iPhone|iPad/i.test(ua + plat)) os = "macos";

  const cta = document.getElementById("heroCta");
  const note = document.getElementById("osNote");
  const grids = {
    macos: document.getElementById("dlMac"),
    windows: document.getElementById("dlWin"),
    linux: document.getElementById("dlLinux"),
  };
  if (!cta || !grids.macos || !grids.windows || !grids.linux) return;

  const render = () => {
    for (const k of Object.keys(grids)) grids[k].hidden = k !== os;
    cta.textContent = QS_T("dl.cta", { os: QS_T(`os.${os}`) });
    if (note) note.textContent = QS_T(`dl.${os === "macos" ? "mac" : os === "windows" ? "win" : "linux"}`);
  };
  render();
  document.addEventListener("qs-lang", render);
  document.body.dataset.os = os;
})();

/* ---------- copy buttons ---------- */
document.querySelectorAll(".copy-btn").forEach((btn) => {
  const render = () => {
    if (!btn.classList.contains("copied")) btn.textContent = QS_T("ui.copy");
  };
  render();
  document.addEventListener("qs-lang", render);
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(btn.dataset.copy);
      btn.textContent = QS_T("ui.copied");
      btn.classList.add("copied");
      setTimeout(() => {
        btn.textContent = QS_T("ui.copy");
        btn.classList.remove("copied");
      }, 1600);
    } catch {
      btn.textContent = QS_T("ui.failed");
    }
  });
});

/* ---------- feature request form → report relay worker ---------- */
(() => {
  const form = document.getElementById("reqForm");
  if (!form) return;
  const status = document.getElementById("reqStatus");
  const ENDPOINT = "https://report.2tsoftware.app";

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const title = document.getElementById("reqTitle").value.trim();
    const note = document.getElementById("reqNote").value.trim();
    const contact = document.getElementById("reqContact").value.trim();
    if (!title || !note) return;
    status.textContent = QS_T("req.sending");
    status.className = "req-status";
    try {
      const r = await fetch(ENDPOINT, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "feature", title, note, contact }),
      });
      const data = r.ok ? await r.json() : null;
      if (!data?.url) throw new Error(`relay ${r.status}`);
      status.innerHTML = `${QS_T("req.sent")} — <a href="${data.url}" target="_blank" rel="noopener">${data.url.replace(/^https?:\/\//, "")}</a>`;
      status.classList.add("ok");
      form.reset();
    } catch {
      status.innerHTML = `<a href="https://github.com/truongnat/quickshort/issues/new?labels=enhancement" target="_blank" rel="noopener">${QS_T("req.fail")}</a>`;
      status.classList.add("err");
    }
  });
})();

/* ---------- supporters wall: docs/supporters.json + self-submit ---------- */
(() => {
  const names = document.getElementById("supNames");
  const form = document.getElementById("supForm");
  if (!names || !form) return;
  const status = document.getElementById("supStatus");
  const ENDPOINT = "https://report.2tsoftware.app";

  const render = async () => {
    try {
      const r = await fetch("supporters.json", { cache: "no-store" });
      const list = r.ok ? (await r.json()).supporters || [] : [];
      names.innerHTML = list.length
        ? list
            .map(
              (s) =>
                `<span class="sup-chip" title="${s.via === "vietqr" ? "VietQR" : "GitHub Sponsors"}">${s.name.replace(/[<>&"]/g, "")}</span>`
            )
            .join("")
        : `<span class="sup-empty">${QS_T("sup.empty")}</span>`;
    } catch {
      names.innerHTML = `<span class="sup-empty">${QS_T("sup.empty")}</span>`;
    }
  };
  render();
  document.addEventListener("qs-lang", render);

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const name = document.getElementById("supName").value.trim();
    if (!name) return;
    status.textContent = QS_T("sup.sending");
    status.className = "req-status";
    try {
      const r = await fetch(ENDPOINT, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ type: "supporter", name }),
      });
      if (!r.ok) throw new Error(`relay ${r.status}`);
      status.textContent = QS_T("sup.sent");
      status.classList.add("ok");
      form.reset();
    } catch {
      status.textContent = QS_T("sup.fail");
      status.classList.add("err");
    }
  });
})();
