# Launch checklist — v1.0.0

Pending work for the v1.0 launch, in dependency order. Decisions already
made: VN-first launch, unsigned builds (no Dev ID yet), winget + AUR as
new channels, metrics via GitHub Traffic + release downloads + report
relay issue volume.

## 1. Windows smoke test (~15 min, blocks the tag)

On a real Windows machine:

- [ ] `winget`-free install: run `Quickshort-Windows-Setup.exe` from
      Releases → SmartScreen **More info → Run anyway**
- [ ] hotkey → region capture → annotate → save
- [ ] album: thumb lands, right-click → **Share Link** → ↗ chip appears
- [ ] right-click again → **Revoke Link** → link returns 404 in browser
- [ ] tray menu → Check for Updates works

## 2. Tag v1.0.0 (after #1)

```bash
git tag -a v1.0.0 -m "v1.0.0 — all three platforms"
git push origin v1.0.0
```

`release.yml` builds dmg+zip / AppImage+deb / NSIS automatically.
After the run, confirm the release shows all 3 platform assets, then mark
it **Latest** and keep v0.x as pre-releases.

## 3. WAF rate-limit rule (~5 min, independent)

Dashboard → zone `2tsoftware.app` → **Security → WAF → Rate limiting
rules → Create rule**:

- Expression:
  `http.host eq "share.2tsoftware.app" and http.request.uri.path eq "/u" and http.request.method eq "POST"`
- Characteristics: `IP`; Period: 60 s; Requests: 60
- Action: **Block**, timeout 10 minutes

(The 30-uploads/day KV quota in the worker stays the real limit; this
just absorbs request floods at the edge.)

## 4. Fill in real checksums + submit packages (after #2)

```bash
# winget — sha256 of the Windows setup exe
curl -sL https://github.com/truongnat/quickshort/releases/download/v1.0.0/Quickshort-Windows-Setup.exe | sha256sum
# AUR — sha256 of the deb
curl -sL https://github.com/truongnat/quickshort/releases/download/v1.0.0/quickshort_amd64.deb | sha256sum
```

- [ ] `packaging/winget/quickshort.installer.yaml` → `InstallerSha256`,
      then `wingetcreate submit` or fork `microsoft/winget-pkgs` and PR
      `manifests/t/truongnat/quickshort/1.0.0/`
- [ ] `packaging/aur/PKGBUILD` → `sha256sums`, publish as
      `quickshort-bin` on aur.archlinux.org (needs an AUR account)

## 5. Demo video (after #2, before posting)

`appshot-video/` renders already (`bun run build`). 60–90 s, VI voice +
EN subs:

1. hotkey → drag region → annotate
2. Share Link → browser opens `share.2tsoftware.app` URL
3. album fan → right-click → Revoke Link
4. end card: `github.com/truongnat/quickshort`

## 6. Launch posts (after #5)

- [ ] WeCommit + J2TEAM + dev-VN groups — video attached
- [ ] Viblo long-form post (only if the video gets traction)

## 7. Measure (week after)

- GitHub repo → **Insights → Traffic** (visitors + referrers)
- `gh api repos/truongnat/quickshort/releases` → `download_count` per asset
- Issue volume from the report relay (`user-report` / `user-request` labels)

## Done already (context)

- `share.2tsoftware.app` + `report.2tsoftware.app` custom domains live;
  share links are revocable (DELETE + delete-key, `shares.json` ledger,
  ↗ chip, Revoke Link menu, card Share→Revoke)
- report relay capped at 10 reports/IP/day
- `packaging/aur/`, `packaging/winget/` manifests committed
- MIT LICENSE
- CI: `verify.yml` runs the album demo under Xvfb (X11) and nested
  Weston (Wayland) every push; macOS verified locally
