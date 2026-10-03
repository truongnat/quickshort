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
  const dlMac = document.getElementById("dlMac");
  const dlOther = document.getElementById("dlOther");
  const title = document.getElementById("otherTitle");
  const desc = document.getElementById("otherDesc");
  const perm = document.getElementById("permNote");
  if (!cta || !dlMac || !dlOther) return;

  const NAMES = { macos: "macOS", windows: "Windows", linux: "Linux" };
  if (os === "macos") {
    cta.textContent = "Download for macOS";
    if (note) note.textContent = "Detected macOS. DMG or Homebrew.";
  } else {
    cta.textContent = `Download for ${NAMES[os]}`;
    dlMac.hidden = true;
    dlOther.hidden = false;
    if (note) note.textContent = `Detected ${NAMES[os]}. No signed binary yet.`;
    if (os === "windows") {
      if (title) title.textContent = "Windows: build from source";
      if (desc) desc.textContent = "Windows backend is compile-checked (GDI BitBlt, Windows.Media.Ocr). A packaged installer is on the roadmap. For now, cargo builds it.";
      if (perm) perm.textContent = "Windows: builds clean from source. macOS DMG + Homebrew cask are the packaged channels today.";
    } else {
      if (title) title.textContent = "Linux: build from source";
      if (desc) desc.textContent = "Linux backend supports X11 and GNOME Wayland (PipeWire). OCR on Linux fails loudly for now, no backend chosen yet.";
      if (perm) perm.textContent = "Linux: X11 + GNOME Wayland capture work from a source build. macOS DMG + Homebrew cask are the packaged channels today.";
    }
  }
  document.body.dataset.os = os;
})();

/* ---------- copy buttons ---------- */
document.querySelectorAll(".copy-btn").forEach((btn) => {
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(btn.dataset.copy);
      btn.textContent = "Copied";
      btn.classList.add("copied");
      setTimeout(() => {
        btn.textContent = "Copy";
        btn.classList.remove("copied");
      }, 1600);
    } catch {
      btn.textContent = "Failed";
    }
  });
});
