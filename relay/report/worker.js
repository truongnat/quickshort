// Quickshort bug-report relay.
//
// The app POSTs diagnostics here; the worker files the issue on
// GitHub with a token held server-side, so users never need a GitHub
// login and the issue lands on the repo under the owner's account.
//
// Deploy:
//   cd relay/report
//   npx wrangler login
//   npx wrangler secret put GITHUB_TOKEN   # fine-grained PAT, repo=quickshort, Issues:rw
//   npx wrangler deploy

const REPO = "truongnat/quickshort";
const MAX_FIELD = 30000;

const str = (v, max) => (typeof v === "string" ? v.slice(0, max) : "");

export default {
  async fetch(req, env) {
    if (req.method !== "POST") {
      return new Response("POST only", { status: 405 });
    }
    let data;
    try {
      data = await req.json();
    } catch {
      return new Response("expected json", { status: 400 });
    }

    const version = str(data.version, 40);
    const os = str(data.os, 120);
    const note = str(data.note, 4000);
    const diagnostics = str(data.diagnostics, MAX_FIELD);
    if (!version || !diagnostics) {
      return new Response("missing version/diagnostics", { status: 400 });
    }

    const title = `User report — v${version} (${os || "unknown OS"})`;
    const body = `${note ? `## Report\n\n${note}\n\n` : ""}${diagnostics}`;

    const gh = await fetch(`https://api.github.com/repos/${REPO}/issues`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.GITHUB_TOKEN}`,
        Accept: "application/vnd.github+json",
        "Content-Type": "application/json",
        "User-Agent": "quickshort-report-relay",
      },
      body: JSON.stringify({ title, body, labels: ["bug", "user-report"] }),
    });
    if (!gh.ok) {
      const detail = await gh.text();
      return new Response(`github ${gh.status}: ${detail.slice(0, 500)}`, {
        status: 502,
      });
    }
    const issue = await gh.json();
    return Response.json({ ok: true, url: issue.html_url });
  },
};
