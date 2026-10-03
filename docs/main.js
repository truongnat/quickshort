// Quickshort landing: dark-cosmic look. Starfield canvas + nebula +
// perspective grid horizon + spotlight bento. GSAP vendored locally.
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

/* ---------- starfield: twinkle + mouse parallax + shooting stars ---------- */
(() => {
  const cv = document.getElementById("stars");
  if (!cv || reduced) return;
  const ctx = cv.getContext("2d");
  let W, H, stars = [], meteors = [], nextMeteor = 0;
  let mx = 0.5, my = 0.5, ox = 0, oy = 0;
  const DPR = Math.min(devicePixelRatio || 1, 2);

  function resize() {
    W = innerWidth; H = innerHeight;
    cv.width = W * DPR; cv.height = H * DPR;
    ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
    const n = Math.min(240, Math.floor((W * H) / 9000));
    stars = Array.from({ length: n }, () => ({
      x: Math.random() * W,
      y: Math.random() * H,
      r: 0.4 + Math.random() * 1.3,
      depth: 0.3 + Math.random() * 0.7, // parallax layer
      phase: Math.random() * Math.PI * 2,
      speed: 0.4 + Math.random() * 1.1,
      cyan: Math.random() < 0.22,
    }));
    nextMeteor = performance.now() + 4000 + Math.random() * 5000;
  }

  addEventListener("resize", resize);
  addEventListener("pointermove", (e) => {
    mx = e.clientX / W; my = e.clientY / H;
  });
  resize();

  function frame(t) {
    ctx.clearRect(0, 0, W, H);
    // parallax eases toward pointer
    ox += ((mx - 0.5) * 26 - ox) * 0.04;
    oy += ((my - 0.5) * 18 - oy) * 0.04;

    for (const s of stars) {
      const tw = 0.45 + 0.55 * Math.abs(Math.sin(t * 0.001 * s.speed + s.phase));
      ctx.globalAlpha = tw * 0.9;
      ctx.fillStyle = s.cyan ? "#9deaff" : "#dfe9ff";
      ctx.beginPath();
      ctx.arc(s.x - ox * s.depth, s.y - oy * s.depth, s.r, 0, 6.2832);
      ctx.fill();
    }

    // occasional shooting star: bright head, fading gradient tail
    if (t > nextMeteor) {
      nextMeteor = t + 6000 + Math.random() * 9000;
      const fromLeft = Math.random() < 0.5;
      meteors.push({
        x: fromLeft ? -60 : W * (0.35 + Math.random() * 0.6),
        y: -20 + Math.random() * H * 0.3,
        vx: (fromLeft ? 1 : -1) * (7 + Math.random() * 4),
        vy: 3.2 + Math.random() * 2,
        life: 1,
      });
    }
    meteors = meteors.filter((m) => m.life > 0);
    for (const m of meteors) {
      m.x += m.vx; m.y += m.vy; m.life -= 0.016;
      const g = ctx.createLinearGradient(m.x, m.y, m.x - m.vx * 12, m.y - m.vy * 12);
      g.addColorStop(0, `rgba(190, 240, 255, ${0.85 * m.life})`);
      g.addColorStop(1, "rgba(190, 240, 255, 0)");
      ctx.globalAlpha = 1;
      ctx.strokeStyle = g;
      ctx.lineWidth = 1.6;
      ctx.beginPath();
      ctx.moveTo(m.x, m.y);
      ctx.lineTo(m.x - m.vx * 12, m.y - m.vy * 12);
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
    requestAnimationFrame(frame);
  }
  requestAnimationFrame(frame);
})();

/* ---------- GSAP choreography ---------- */
if (!reduced && window.gsap) {
  gsap.registerPlugin(ScrollTrigger);

  // kinetic headline: masked words slide up with a soft overshoot
  gsap.to(".headline .line > span", {
    y: 0,
    duration: 1.1,
    stagger: 0.13,
    ease: "back.out(1.3)",
    delay: 0.2,
  });

  gsap.from(".sub, .cta-row", {
    opacity: 0,
    y: 26,
    duration: 0.9,
    stagger: 0.12,
    ease: "power3.out",
    delay: 0.7,
  });

  gsap.from(".eyebrow", {
    opacity: 0,
    scale: 0.85,
    duration: 0.7,
    ease: "back.out(2)",
  });

  // hero card entrance + idle float, on the WRAPPER so the inner
  // card's transform stays free for pointer tilt
  gsap.from("#floatWrap", {
    opacity: 0,
    y: 90,
    scale: 0.94,
    duration: 1.3,
    ease: "power3.out",
    delay: 0.5,
  });
  gsap.to("#floatWrap", {
    y: -13,
    duration: 3.6,
    ease: "sine.inOut",
    yoyo: true,
    repeat: -1,
    delay: 2,
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
  let rx = 4, ry = 0, tx = 4, ty = 0;

  stage.addEventListener("pointermove", (e) => {
    const r = stage.getBoundingClientRect();
    const nx = (e.clientX - r.left) / r.width - 0.5;
    const ny = (e.clientY - r.top) / r.height - 0.5;
    tx = 4 + ny * -8;
    ty = nx * 10;
  });
  stage.addEventListener("pointerleave", () => { tx = 4; ty = 0; });

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

/* ---------- card tilt + spotlight tracking ---------- */
(() => {
  if (reduced) return;
  document.querySelectorAll("[data-tilt]").forEach((el) => {
    el.addEventListener("pointermove", (e) => {
      const r = el.getBoundingClientRect();
      const px = e.clientX - r.left;
      const py = e.clientY - r.top;
      el.style.setProperty("--mx", `${px}px`);
      el.style.setProperty("--my", `${py}px`);
      const nx = px / r.width - 0.5;
      const ny = py / r.height - 0.5;
      el.style.transform = `translateY(-8px) rotateX(${ny * -6}deg) rotateY(${nx * 8}deg)`;
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
