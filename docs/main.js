// Quickshort landing: particle field, 3D tilt, kinetic type, scroll reveals.
// Motion is gated on prefers-reduced-motion; GSAP falls back to plain CSS reveals.

document.documentElement.classList.replace("no-js", "js");

const isStatic = new URLSearchParams(location.search).has("static");
const reduced =
  window.matchMedia("(prefers-reduced-motion: reduce)").matches || isStatic;
if (isStatic) {
  document.documentElement.style.scrollBehavior = "auto";
  const hero = document.querySelector(".hero");
  if (hero) hero.style.minHeight = "auto";
}

/* ---------- particle field (canvas, cheap, no lib) ---------- */
(() => {
  const cv = document.getElementById("particles");
  const ctx = cv.getContext("2d");
  let w, h, pts = [], mx = -1e4, my = -1e4;

  function resize() {
    w = cv.width = innerWidth * devicePixelRatio;
    h = cv.height = innerHeight * devicePixelRatio;
    cv.style.width = innerWidth + "px";
    cv.style.height = innerHeight + "px";
    const n = reduced ? 0 : Math.min(90, Math.floor(innerWidth / 16));
    pts = Array.from({ length: n }, () => ({
      x: Math.random() * w,
      y: Math.random() * h,
      vx: (Math.random() - 0.5) * 0.14 * devicePixelRatio,
      vy: (Math.random() - 0.5) * 0.14 * devicePixelRatio,
      r: (Math.random() * 1.4 + 0.5) * devicePixelRatio,
    }));
  }
  resize();
  addEventListener("resize", resize);
  addEventListener("pointermove", (e) => {
    mx = e.clientX * devicePixelRatio;
    my = e.clientY * devicePixelRatio;
  });

  const LINK = 130 * devicePixelRatio;
  function frame() {
    ctx.clearRect(0, 0, w, h);
    for (const p of pts) {
      p.x += p.vx; p.y += p.vy;
      if (p.x < 0 || p.x > w) p.vx *= -1;
      if (p.y < 0 || p.y > h) p.vy *= -1;
      // gentle repel from cursor
      const dx = p.x - mx, dy = p.y - my, d2 = dx * dx + dy * dy;
      if (d2 < LINK * LINK * 0.7) {
        const d = Math.sqrt(d2) || 1;
        p.x += (dx / d) * 0.6;
        p.y += (dy / d) * 0.6;
      }
      ctx.beginPath();
      ctx.arc(p.x, p.y, p.r, 0, Math.PI * 2);
      ctx.fillStyle = "rgba(103, 168, 255, 0.5)";
      ctx.fill();
    }
    for (let i = 0; i < pts.length; i++) {
      for (let j = i + 1; j < pts.length; j++) {
        const a = pts[i], b = pts[j];
        const dx = a.x - b.x, dy = a.y - b.y;
        const d2 = dx * dx + dy * dy;
        if (d2 < LINK * LINK) {
          ctx.strokeStyle = `rgba(79, 124, 255, ${0.14 * (1 - d2 / (LINK * LINK))})`;
          ctx.lineWidth = devicePixelRatio * 0.7;
          ctx.beginPath();
          ctx.moveTo(a.x, a.y);
          ctx.lineTo(b.x, b.y);
          ctx.stroke();
        }
      }
    }
    if (!reduced) requestAnimationFrame(frame);
  }
  if (!reduced) frame();
})();

/* ---------- GSAP choreography ---------- */
if (!reduced && window.gsap) {
  gsap.registerPlugin(ScrollTrigger);

  // kinetic headline: masked lines slide up
  gsap.to(".headline .line > span", {
    y: 0,
    duration: 1.05,
    stagger: 0.13,
    ease: "power4.out",
    delay: 0.15,
  });

  gsap.from(".sub, .cta-row", {
    opacity: 0,
    y: 26,
    duration: 0.9,
    stagger: 0.12,
    ease: "power3.out",
    delay: 0.6,
  });

  gsap.from(".eyebrow", {
    opacity: 0,
    x: -18,
    duration: 0.8,
    ease: "power3.out",
  });

  // hero card entrance + idle float
  gsap.from("#shotCard", {
    opacity: 0,
    rotateY: -40,
    z: -160,
    duration: 1.4,
    ease: "power3.out",
    delay: 0.35,
  });
  gsap.to("#shotCard", {
    y: -12,
    duration: 3.2,
    ease: "sine.inOut",
    yoyo: true,
    repeat: -1,
    delay: 1.8,
  });

  // crop corners snap in like a selection being committed
  gsap.to(".crop-corner", {
    opacity: 1,
    duration: 0.4,
    stagger: 0.07,
    ease: "power2.out",
    delay: 1.5,
  });
  gsap.to(".crop-corner", {
    scale: 1.08,
    duration: 1.6,
    ease: "sine.inOut",
    yoyo: true,
    repeat: -1,
    delay: 2,
  });

  // self-drawing selection loops forever
  const sel = document.getElementById("dragSel");
  const selLoop = gsap.timeline({ repeat: -1, repeatDelay: 2.4, delay: 2.2 });
  selLoop
    .set(sel, { left: "12%", top: "14%", width: 0, height: 0, opacity: 1 })
    .to(sel, { width: "72%", height: "58%", duration: 0.9, ease: "power2.inOut" })
    .to(sel, { opacity: 0, duration: 0.4, ease: "power1.out" }, "+=1.1");

  // scroll reveals
  document.querySelectorAll(".reveal").forEach((el, i) => {
    gsap.to(el, {
      opacity: 1,
      y: 0,
      duration: 0.9,
      ease: "power3.out",
      scrollTrigger: { trigger: el, start: "top 88%" },
      delay: (i % 4) * 0.05,
    });
  });

  // marquee
  gsap.to(".marquee-track", {
    xPercent: -50,
    duration: 26,
    ease: "none",
    repeat: -1,
  });

  // pipeline step badges pop in
  gsap.utils.toArray(".pipe-num").forEach((el) => {
    gsap.from(el, {
      scale: 0,
      duration: 0.6,
      ease: "back.out(2.2)",
      scrollTrigger: { trigger: el, start: "top 90%" },
    });
  });
} else {
  // reduced motion / no GSAP: everything simply visible
  document.querySelectorAll(".reveal").forEach((el) => el.classList.add("shown"));
  document.querySelectorAll(".headline .line > span").forEach((el) => {
    el.style.transform = "none";
  });
  document.querySelectorAll(".crop-corner").forEach((el) => (el.style.opacity = 1));
}

/* ---------- pointer 3D tilt on the hero card ---------- */
(() => {
  if (reduced) return;
  const stage = document.getElementById("stage");
  const card = document.getElementById("shotCard");
  if (!stage || !card) return;
  let rx = -14, ry = 6, tx = -14, ty = 6;

  stage.addEventListener("pointermove", (e) => {
    const r = stage.getBoundingClientRect();
    const nx = (e.clientX - r.left) / r.width - 0.5;
    const ny = (e.clientY - r.top) / r.height - 0.5;
    tx = -14 + ny * -10;
    ty = 6 + nx * 14;
  });
  stage.addEventListener("pointerleave", () => { tx = -14; ty = 6; });

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
    const strength = 18;
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

/* ---------- feature card tilt ---------- */
(() => {
  if (reduced) return;
  document.querySelectorAll("[data-tilt]").forEach((el) => {
    el.addEventListener("pointermove", (e) => {
      const r = el.getBoundingClientRect();
      const nx = (e.clientX - r.left) / r.width - 0.5;
      const ny = (e.clientY - r.top) / r.height - 0.5;
      el.style.transform = `translateY(-6px) rotateX(${ny * -4}deg) rotateY(${nx * 5}deg)`;
    });
    el.addEventListener("pointerleave", () => {
      el.style.transform = "";
    });
  });
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
