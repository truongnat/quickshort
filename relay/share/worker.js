// quickshort-share — image share-link relay.
//
// POST /u  (image/png body) -> { url } — stores the PNG in KV for 7 days
// GET  /s/<id>.png          -> the PNG
//
// Abuse controls: 12 MB cap, 30 uploads/IP/day soft limit. The client
// binary is public so no real auth is possible; unguessable 128-bit ids
// keep shared files private by obscurity (same guarantee as before).
const MAX_BYTES = 12 * 1024 * 1024;
const TTL_S = 7 * 24 * 3600;
const RATE_PER_DAY = 30;

export default {
  async fetch(req, env, ctx) {
    const url = new URL(req.url);
    if (req.method === "POST" && url.pathname === "/u") return upload(req, env, ctx, url);
    if (req.method === "GET" && url.pathname.startsWith("/s/")) return serve(url, env);
    if (url.pathname === "/healthz") return new Response("ok");
    return new Response("not found", { status: 404 });
  },
};

async function upload(req, env, ctx, url) {
  const ct = req.headers.get("content-type") || "";
  if (!ct.startsWith("image/png")) return err(415, "image/png only");
  if (Number(req.headers.get("content-length") || 0) > MAX_BYTES) {
    return err(413, "too large");
  }
  const ip = req.headers.get("cf-connecting-ip") || "unknown";
  const rk = `rl:${ip}:${new Date().toISOString().slice(0, 10)}`;
  const n = Number((await env.SHARE.get(rk)) || 0);
  if (n >= RATE_PER_DAY) return err(429, "daily share limit reached");

  const body = await req.arrayBuffer();
  if (!body.byteLength || body.byteLength > MAX_BYTES) return err(413, "bad size");
  const id = crypto.randomUUID().replaceAll("-", "");
  await env.SHARE.put(`img:${id}`, body, { expirationTtl: TTL_S });
  ctx.waitUntil(env.SHARE.put(rk, String(n + 1), { expirationTtl: 2 * 86400 }));
  return Response.json({ url: `${url.origin}/s/${id}.png`, expires_in: TTL_S });
}

async function serve(url, env) {
  const m = /^\/s\/([a-f0-9]{32})\.png$/.exec(url.pathname);
  if (!m) return new Response("not found", { status: 404 });
  const img = await env.SHARE.get(`img:${m[1]}`, "arrayBuffer");
  if (!img) return new Response("gone", { status: 404 });
  return new Response(img, {
    headers: {
      "content-type": "image/png",
      "cache-control": "public, max-age=86400",
      "x-content-type-options": "nosniff",
    },
  });
}

function err(status, msg) {
  return Response.json({ error: msg }, { status });
}
