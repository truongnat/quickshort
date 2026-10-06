// Quickshort feedback relay — bug reports and feature requests.
//
// The app POSTs diagnostics here and the landing-page form POSTs
// feature requests; the worker files the issue on GitHub with a token
// held server-side, so users never need a GitHub login and the issue
// lands on the repo under the owner's account.
//
// Deploy:
//   cd relay/report
//   npx wrangler login
//   npx wrangler secret put GITHUB_TOKEN   # fine-grained PAT, repo=quickshort, Issues:rw
//   npx wrangler deploy

const REPO = "truongnat/quickshort";
const MAX_FIELD = 30000;
const ALLOW_ORIGIN = "https://truongnat.github.io";

const str = (v, max) => (typeof v === "string" ? v.slice(0, max) : "");

const cors = (res) => {
  res.headers.set("Access-Control-Allow-Origin", ALLOW_ORIGIN);
  res.headers.set("Access-Control-Allow-Methods", "POST, OPTIONS");
  res.headers.set("Access-Control-Allow-Headers", "Content-Type");
  return res;
};

export default {
  async fetch(req, env) {
    if (req.method === "OPTIONS") {
      return cors(new Response(null, { status: 204 }));
    }
    if (req.method !== "POST") {
      return cors(new Response("POST only", { status: 405 }));
    }
    let data;
    try {
      data = await req.json();
    } catch {
      return cors(new Response("expected json", { status: 400 }));
    }

    let title, body, labels;
    if (str(data.type, 20) === "feature") {
      const ft = str(data.title, 120).trim();
      const note = str(data.note, 8000).trim();
      const contact = str(data.contact, 200).trim();
      if (!ft || !note) {
        return cors(new Response("missing title/note", { status: 400 }));
      }
      title = `Feature: ${ft}`;
      body = `${note}\n\n---\n_Submitted via quickshort feedback form_${
        contact ? ` · contact: ${contact}` : ""
      }`;
      labels = ["enhancement", "user-request"];
    } else {
      const version = str(data.version, 40);
      const os = str(data.os, 120);
      const note = str(data.note, 4000);
      const diagnostics = str(data.diagnostics, MAX_FIELD);
      if (!version || !diagnostics) {
        return cors(
          new Response("missing version/diagnostics", { status: 400 })
        );
      }
      title = `User report — v${version} (${os || "unknown OS"})`;
      body = `${note ? `## Report\n\n${note}\n\n` : ""}${diagnostics}`;
      labels = ["bug", "user-report"];
    }

    const gh = await fetch(`https://api.github.com/repos/${REPO}/issues`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.GITHUB_TOKEN}`,
        Accept: "application/vnd.github+json",
        "Content-Type": "application/json",
        "User-Agent": "quickshort-report-relay",
      },
      body: JSON.stringify({ title, body, labels }),
    });
    if (!gh.ok) {
      const detail = await gh.text();
      return cors(
        new Response(`github ${gh.status}: ${detail.slice(0, 500)}`, {
          status: 502,
        })
      );
    }
    const issue = await gh.json();
    return cors(Response.json({ ok: true, url: issue.html_url }));
  },
};
