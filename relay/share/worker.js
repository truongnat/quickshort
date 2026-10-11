// quickshort-share — image share-link relay.
//
// POST   /u            -> { url, delete_key, expires_in } — stores the
//                         image in KV for 3 days. Body is image/webp
//                         (current clients) or image/png (older builds).
// GET    /s/<id>.<ext> -> the image (ext = webp|png)
// DELETE /s/<id>.<ext> -> revoke early; needs the X-Delete-Key header
//                         that only the uploader received. KV stores
//                         SHA-256(key), never the key itself — a KV
//                         leak doesn't grant delete rights.
//
// Abuse controls: 12 MB cap, 30 uploads/IP/day soft limit. The client
// binary is public so no real auth is possible; unguessable 128-bit ids
// keep shared files private by obscurity.
const MAX_BYTES = 12 * 1024 * 1024;
const TTL_S = 3 * 24 * 3600;
const RATE_PER_DAY = 30;
const TYPES = { "image/webp": "webp", "image/png": "png" };

export default {
  async fetch(req, env, ctx) {
    const url = new URL(req.url);
    if (req.method === "POST" && url.pathname === "/u") return upload(req, env, ctx, url);
    if (url.pathname.startsWith("/s/")) {
      if (req.method === "GET") return serve(url, env);
      if (req.method === "DELETE") return revoke(req, url, env);
    }
    if (url.pathname === "/healthz") return new Response("ok");
    return new Response("not found", { status: 404 });
  },
};

const shareKey = (m) => `${m[2]}:${m[1]}`;

// matches GET /s/<32-hex>.<webp|png> — null for anything else
function parsePath(url) {
  return /^\/s\/([a-f0-9]{32})\.(webp|png)$/.exec(url.pathname);
}

async function sha256hex(text) {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(text)
  );
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

async function upload(req, env, ctx, url) {
  const ct = (req.headers.get("content-type") || "").split(";")[0].trim();
  const ext = TYPES[ct];
  if (!ext) return err(415, "image/webp or image/png only");
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
  const deleteKey = crypto.randomUUID().replaceAll("-", "");
  // type is in the key so serve() can reply with the right content-type
  // without a second KV read; the delete-key hash rides in metadata
  await env.SHARE.put(`${ext}:${id}`, body, {
    expirationTtl: TTL_S,
    metadata: { dk: await sha256hex(deleteKey) },
  });
  ctx.waitUntil(env.SHARE.put(rk, String(n + 1), { expirationTtl: 2 * 86400 }));
  return Response.json({
    url: `${url.origin}/s/${id}.${ext}`,
    delete_key: deleteKey,
    expires_in: TTL_S,
  });
}

async function serve(url, env) {
  const m = parsePath(url);
  if (!m) return new Response("not found", { status: 404 });
  const img = await env.SHARE.get(shareKey(m), "arrayBuffer");
  if (!img) return new Response("gone", { status: 404 });
  return new Response(img, {
    headers: {
      "content-type": `image/${m[2]}`,
      "cache-control": "public, max-age=86400",
      "x-content-type-options": "nosniff",
    },
  });
}

async function revoke(req, url, env) {
  const m = parsePath(url);
  if (!m) return new Response("not found", { status: 404 });
  const key = req.headers.get("x-delete-key") || "";
  if (!/^[a-f0-9]{32}$/.test(key)) return err(403, "bad delete key");
  const { metadata } = await env.SHARE.getWithMetadata(shareKey(m));
  if (!metadata?.dk) return new Response("gone", { status: 404 });
  // constant-time compare is overkill at this threat level but cheap
  if ((await sha256hex(key)) !== metadata.dk) return err(403, "bad delete key");
  await env.SHARE.delete(shareKey(m));
  return Response.json({ ok: true });
}

function err(status, msg) {
  return Response.json({ error: msg }, { status });
}
