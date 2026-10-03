// Quickshort landing: liquid-glass look, GSAP choreography, pointer tilt.
// Motion is gated on prefers-reduced-motion; ?static renders a flat page
// for screenshots/tests; without JS the .no-js class shows everything.

document.documentElement.classList.replace("no-js", "js");

const isStatic = new URLSearchParams(location.search).has("static");
const reduced =
  window.matchMedia("(prefers-reduced-motion: reduce)").matches || isStatic;
if (isStatic) {
  document.documentElement.style.scrollBehavior = "auto";
  const hero = document.querySelector(".hero");
  if (hero) hero.style.minHeight = "auto";
}

/* ---------- GSAP choreography ---------- */
if (!reduced && window.gsap) {
  gsap.registerPlugin(ScrollTrigger);

  // kinetic headline: masked lines slide up with a soft overshoot
  gsap.to(".headline .line > span", {
    y: 0,
    duration: 1.1,
    stagger: 0.14,
    ease: "back.out(1.3)",
    delay: 0.15,
  });

  gsap.from(".sub, .cta-row", {
    opacity: 0,
    y: 26,
    duration: 0.9,
    stagger: 0.12,
    ease: "power3.out",
    delay: 0.65,
  });

  gsap.from(".eyebrow", {
    opacity: 0,
    scale: 0.7,
    duration: 0.7,
    ease: "back.out(2)",
  });

  // hero card entrance + idle float — on the WRAPPER so the inner
  // card's transform stays free for pointer tilt
  gsap.from("#floatWrap", {
    opacity: 0,
    rotateY: -38,
    z: -180,
    scale: 0.92,
    duration: 1.4,
    ease: "power3.out",
    delay: 0.4,
  });
  gsap.to("#floatWrap", {
    y: -14,
    duration: 3.4,
    ease: "sine.inOut",
    yoyo: true,
    repeat: -1,
    delay: 1.9,
  });

  // crop corners pop in like a selection being committed
  gsap.to(".crop-corner", {
    opacity: 1,
    duration: 0.45,
    stagger: 0.08,
    ease: "back.out(2.5)",
    delay: 1.5,
  });
  gsap.to(".crop-corner", {
    scale: 1.1,
    duration: 1.7,
    ease: "sine.inOut",
    yoyo: true,
    repeat: -1,
    delay: 2.1,
  });

  // self-drawing selection loops forever
  const sel = document.getElementById("dragSel");
  gsap.timeline({ repeat: -1, repeatDelay: 2.6, delay: 2.4 })
    .set(sel, { left: "12%", top: "14%", width: 0, height: 0, opacity: 1 })
    .to(sel, { width: "70%", height: "56%", duration: 1, ease: "power2.inOut" })
    .to(sel, { opacity: 0, duration: 0.4, ease: "power1.out" }, "+=1.2");

  // pipeline step badges pop in
  gsap.utils.toArray(".pipe-num").forEach((el) => {
    gsap.from(el, {
      scale: 0,
      rotate: -40,
      duration: 0.7,
      ease: "back.out(2.4)",
      scrollTrigger: { trigger: el, start: "top 90%" },
    });
  });

  // glass orbs drift on scroll (parallax)
  gsap.to(".orb.o1", {
    y: -90,
    scrollTrigger: { trigger: ".hero", start: "top top", end: "bottom top", scrub: 1 },
  });
  gsap.to(".orb.o2", {
    y: 70,
    scrollTrigger: { trigger: ".hero", start: "top top", end: "bottom top", scrub: 1 },
  });
} else {
  document.querySelectorAll(".reveal").forEach((el) => el.classList.add("shown"));
  document.querySelectorAll(".headline .line > span").forEach((el) => {
    el.style.transform = "none";
  });
  document.querySelectorAll(".crop-corner").forEach((el) => (el.style.opacity = 1));
}

/* ---------- scroll reveals: IntersectionObserver + CSS transition ---------- */
/* (kept off GSAP so it never writes an inline transform that would
    fight the fcard :hover / tilt transforms) */
if (!reduced && "IntersectionObserver" in window) {
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((e, i) => {
        if (!e.isIntersecting) return;
        const el = e.target;
        el.style.transitionDelay = `${(i % 4) * 70}ms`;
        el.classList.add("shown");
        setTimeout(() => (el.style.transitionDelay = ""), 1200);
        io.unobserve(el);
      });
    },
    { rootMargin: "0px 0px -10% 0px" }
  );
  document.querySelectorAll(".reveal").forEach((el) => io.observe(el));
} else if (window.gsap === undefined) {
  document.querySelectorAll(".reveal").forEach((el) => el.classList.add("shown"));
}

/* ---------- pointer 3D tilt on the hero card ---------- */
(() => {
  if (reduced) return;
  const stage = document.getElementById("stage");
  const card = document.getElementById("shotCard");
  if (!stage || !card) return;
  let rx = 6, ry = -13, tx = 6, ty = -13;

  stage.addEventListener("pointermove", (e) => {
    const r = stage.getBoundingClientRect();
    const nx = (e.clientX - r.left) / r.width - 0.5;
    const ny = (e.clientY - r.top) / r.height - 0.5;
    tx = 6 + ny * -10;
    ty = -13 + nx * 14;
  });
  stage.addEventListener("pointerleave", () => { tx = 6; ty = -13; });

  (function tilt() {
    rx += (tx - rx) * 0.08;
    ry += (ty - ry) * 0.08;
    card.style.transform = `rotateX(${rx}deg) rotateY(${ry}deg)`;
    requestAnimationFrame(tilt);
  })();
})();

/* ---------- magnetic buttons ---------- */
(() => {
  if (reduced) return;
  document.querySelectorAll(".magnetic").forEach((el) => {
    const strength = 16;
    el.addEventListener("pointermove", (e) => {
      const r = el.getBoundingClientRect();
      const x = (e.clientX - r.left - r.width / 2) / (r.width / 2);
      const y = (e.clientY - r.top - r.height / 2) / (r.height / 2);
      el.style.transform = `translate(${x * strength}px, ${y * strength * 0.6}px)`;
    });
    el.addEventListener("pointerleave", () => {
      el.style.transform = "";
    });
  });
})();

/* ---------- feature card jelly tilt ---------- */
(() => {
  if (reduced) return;
  document.querySelectorAll("[data-tilt]").forEach((el) => {
    el.addEventListener("pointermove", (e) => {
      const r = el.getBoundingClientRect();
      const nx = (e.clientX - r.left) / r.width - 0.5;
      const ny = (e.clientY - r.top) / r.height - 0.5;
      el.style.transform = `translateY(-10px) scale(1.02) rotateX(${ny * -8}deg) rotateY(${nx * 10}deg)`;
    });
    el.addEventListener("pointerleave", () => {
      el.style.transform = "";
    });
  });
})();

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
    if (note) note.textContent = "Detected macOS — DMG or Homebrew.";
  } else {
    cta.textContent = `Download for ${NAMES[os]}`;
    dlMac.hidden = true;
    dlOther.hidden = false;
    if (note) note.textContent = `Detected ${NAMES[os]} — no signed binary yet.`;
    if (os === "windows") {
      if (title) title.textContent = "Windows — build from source";
      if (desc) desc.textContent = "Windows backend is compile-checked (GDI BitBlt, Windows.Media.Ocr). A packaged installer is on the roadmap — for now, cargo builds it.";
      if (perm) perm.textContent = "Windows: builds clean from source. macOS DMG + Homebrew cask are the packaged channels today.";
    } else {
      if (title) title.textContent = "Linux — build from source";
      if (desc) desc.textContent = "Linux backend supports X11 and GNOME Wayland (PipeWire). OCR on Linux fails loudly for now — no backend chosen yet.";
      if (perm) perm.textContent = "Linux: X11 + GNOME Wayland capture work from a source build. macOS DMG + Homebrew cask are the packaged channels today.";
    }
    // show the mac card too, dimmed, so mac visitors landing here still see it
    const other = dlOther.querySelector(".dl-card");
    if (other) other.classList.add("fcard-wide");
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
