# Bug-report relay

Cloudflare Worker that files GitHub issues on behalf of the app, so
users can report a bug without a GitHub account. The worker holds a
token server-side; issues land on `truongnat/quickshort` under the
token owner's account with labels `bug` + `user-report`.

## One-time deploy

```sh
cd relay/report
npx wrangler login                                  # browser auth, free tier is fine
npx wrangler secret put GITHUB_TOKEN                # see below
npx wrangler deploy                                 # prints https://quickshort-report.<sub>.workers.dev
```

`GITHUB_TOKEN`: a **fine-grained** PAT scoped to repository
`truongnat/quickshort` only, permission **Issues: Read and write**.
Nothing else — that keeps the blast radius of a leaked Worker config
to issue spam, which GitHub rate-limits anyway.

Then point the app at it — either edit `REPORT_ENDPOINT` in
`crates/qs-app/src/actions.rs`, or run with `QS_REPORT_URL=<worker url>`.

## Request/response

`POST /` with JSON:

```json
{ "version": "0.3.1", "os": "macos aarch64", "diagnostics": "...", "note": "optional" }
```

Returns `{ "ok": true, "url": "https://github.com/.../issues/N" }`;
the app opens that URL so the reporter can watch the issue.
