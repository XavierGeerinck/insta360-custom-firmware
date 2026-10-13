# Public site and going public — design

Date: 2026-10-13. Branch: `feat/public-site` (off `feat/m3`).

## Goal

Publish the project: make the repository public without leaking personal data, and
give it a GitHub Pages site that explains what the firmware is, what it does, how it
works and how to install it, with every reverse-engineering and hardware document
browsable and searchable.

Readers: Wave owners who are comfortable with a terminal and want the telemetry
gone, and the tinkerers and researchers who come for the protocol and RE material.

## Settled decisions

| # | Decision |
|---|---|
| 1 | Generator: Astro Starlight, in `site/`. |
| 2 | Markdown stays where it lives (`docs/`, `waved/docs/`). The site copies an explicit allowlist at build time. There is no second copy in git. |
| 3 | Install documents today's flow (build from source, ADB, `install.sh`). Prebuilt GitHub Releases come later; the install page is laid out so that path slots in above the source build. `install.sh` and `revert.sh` are brought up to the device-proven procedure first (see "Install and revert scripts"): as of 2026-10-13 they would leave a stranger without ADB after the first reboot. |
| 4 | `decompiled/*.c` (Ghidra output of Insta360 binaries) is removed from the tree and from history. Every report, finding and line reference built on it stays, as prose. `ghidra_decompile.*` stays so others can reproduce it. |
| 5 | Personal data is scrubbed from the tree, then history is rewritten. |
| 6 | The rewritten history goes to a **new** public repository. The current private repository is renamed to `insta360-custom-firmware-archive` and stays private. Force-pushing in place would leave the old commits reachable by SHA on a public repo. |
| 7 | Every outward step (rename, push, visibility, enabling Pages) waits for an explicit go at that moment. |

## Site map

**Landing page** (`site/src/content/docs/index.mdx`, Starlight `splash` template)

- Hero: "Open firmware for the Insta360 Wave", one sentence, buttons Install · Docs · GitHub.
- Why: what stock firmware does (35+ telemetry event types to ByteDance's DataFinder,
  recording uploads to Insta360 servers, RSA-512 update signing, root telnet), each claim
  linking to the Security Q&A.
- What you get, as cards: zero telemetry; local recordings with your own webhook; macOS
  speakerphone (UAC2) with echo cancellation; Bluetooth audio and calls; Wave Controller
  compatibility; signed self-hosted updates; touchscreen UI; one-script revert to stock.
- Screens: about six PNGs from `waved/crates/waved-ui/golden/`. These are rendered by the
  UI code itself, not the design mockups.
- How it works: one diagram, linking to the architecture page.
- Note: unofficial, not affiliated with Insta360, reversible, use at your own risk.

**Sidebar**

| Group | Pages | Source |
|---|---|---|
| Getting started | Overview · Requirements · Install · First setup · Revert to stock · Troubleshooting | new, in `site/src/content/docs/start/` |
| Guides | Features tour (new, `features.md`) · Wi-Fi & provisioning · Recording webhooks · Updates | new + `waved/docs/WIFI.md`, `WEBHOOKS.md`, `UPDATES.md` |
| How it works | Architecture · Hardware overview | new, in `site/src/content/docs/how/` |
| Reference | Audio hardware · Compute hardware · Display · MCU protocol · Bluetooth protocol · Wave Controller protocol · UI coordinate system | `waved/docs/*.md`, `waved/docs/ui/COORDINATE_SYSTEM.md` |
| Research | RE report · Security Q&A · Device dump · AI models · NPU · UI · Firmware gap analysis · Debug: latency · Debug: pipeline · Device access & extraction (the three `GUIDE_*.md`) · Analysis reports (9) · Stock UI (assets, home screen) · NPU research (all of `docs/npu/**`) | `docs/*.md`, `docs/analysis_reports/`, `waved/docs/ui/ASSETS.md`, `HOME_SCREEN.md`, `docs/npu/` |
| Development | Building · Testing (the five layers) · Project structure · Contributing | new, from the README and `waved/` |

**Not on the site:** `docs/superpowers/`, `waved/docs/superpowers/`, `TODO.md`,
`waved/TODO.md`, `waved/CLAUDE.md`, `docs/mockups/`, crate READMEs. They stay in the
repository; links to them become GitHub links.

New pages are written from the code and the existing docs. Every behavioural claim
must be traceable to one of them; nothing describes a feature that is not on the
branch that publishes.

## Site architecture

```
site/
├── astro.config.mjs        site + base, Starlight config, sidebar, plugins
├── package.json            scripts: sync, dev, build, preview, test
├── sync/manifest.mjs       allowlist: source path → slug + sidebar group
├── scripts/sync-docs.mjs   copies + rewrites synced docs
├── scripts/sync-docs.test.mjs
├── src/content/docs/       hand-written pages (committed)
│   ├── index.mdx  features.md
│   ├── start/  how/  dev/
│   └── guides/  reference/  research/   ← generated, gitignored
└── src/assets/screens/     curated golden PNGs (copied by sync)
```

`npm run dev` and `npm run build` run `sync` first.

### Sync script

For each manifest entry (a file, or the `docs/npu/**` glob):

1. **Read** the source. A missing allowlisted file is a hard error, so a rename cannot
   silently drop a page.
2. **Front matter.** The first `# H1` becomes `title` and is removed from the body.
   `editUrl` points at the source file on GitHub. Existing front matter (none today) is kept.
3. **Links.** Each relative Markdown link is resolved against the source file's
   directory, then:

   | Target | Becomes |
   |---|---|
   | another synced doc (with `#anchor`) | its site URL, anchor kept |
   | under `decompiled/` | plain text (link removed, label kept) |
   | any other path in the repository (code, `.json`, specs, `TODO.md`) | `https://github.com/<owner>/<repo>/blob/main/<path>` (`tree/` for directories) |
   | `http(s):`, `mailto:`, pure `#anchor` | unchanged |

4. **Images.** Relative image references are copied next to the generated page and
   rewritten to match.
5. **Write** to `src/content/docs/<slug>.md`.

The owner/repo pair comes from one constant in `astro.config.mjs`, so the new
repository's name is one edit away.

### Rendering

- Mermaid: `astro-mermaid` renders client-side (29 diagrams in `DEBUG_PIPELINE.md`,
  `REPORT_NPU.md`, `REPORT_AI_MODELS.md`). `mermaid` is pinned to `^11`, which is the
  plugin's peer range. No headless browser in CI.
- Raw HTML already in the docs (`<details>`, `<img>`) passes through `.md` unchanged.
- `starlight-links-validator` fails the build on a broken internal link or anchor.
- Search is Starlight's built-in Pagefind index.

## Deploy

`.github/workflows/pages.yml`:

- Triggers: push to `main` touching `docs/**`, `waved/docs/**`, `site/**`,
  `waved/crates/waved-ui/golden/**` or the workflow file; `workflow_dispatch`;
  `pull_request` (build only, no deploy).
- Job `build`: checkout, Node `lts/*`, `npm ci` + `npm test` + `npm run build` in `site/`,
  `actions/upload-pages-artifact` with `site/dist`.
- Job `deploy` (push and dispatch only): `actions/deploy-pages`, environment
  `github-pages`, permissions `pages: write`, `id-token: write`.
- Concurrency group `pages`, cancel in progress.
- Pages source: "GitHub Actions". URL: `https://xaviergeerinck.github.io/insta360-custom-firmware/`.
  A custom domain later is the `site` setting plus a DNS record.

## Repository hygiene

- `README.md` is rewritten: what it is, the why in three lines, a link to the site, a
  short build/install pointer, status, license. The current one dates from April and
  predates the UI, Bluetooth, Wave Controller and update work.
- `LICENSE`: MIT (the README already claims it; there is no file). Copyright Xavier Geerinck.
- `.gitignore`: `site/node_modules/`, `site/dist/`, `site/.astro/`, the generated content
  directories.

## Install and revert scripts

Found while planning (amendment, 2026-10-13). `waved/deploy/install.sh` was last changed on
2026-04-10 and no longer matches the procedure in `waved/docs/DISPLAY_HARDWARE.md#deploy`:

- **ADB is lost after the first reboot.** Stock `S89usbgadget` keeps `adbd_enable` commented
  out at boot; stock `pspapp` turns ADB on later, and under waved `pspapp` never runs. waved
  has no ADB-enable path of its own, and the HID `0xC0` enable goes through `pspapp` too. A
  stranger who installs and reboots can no longer run `revert.sh`. The maintainer's Wave
  works only because its `S89usbgadget` was edited by hand
  (`docs/GUIDE_DEVICE_ACCESS.md`, "Making ADB Survive a `pspapp` Failure").
- It pushes the unstripped binary straight over `/data/waved/waved` (the documented failure
  that leaves no binary), checks for an existing `config.toml` on the host instead of the
  Wave, and skips `rsyslog.conf` (without it stock logging fills `/data`), `S03splash`,
  `splash.fb` and the USB gadget patch (`waved/scripts/patch-stock-usbgadget.sh`).
- `revert.sh` leaves `S03splash`, `rsyslog.conf` and the patched `S89usbgadget` in the overlay
  (`waved/docs/UPDATES.md`, "Paths back").

Changes:

- `patch-stock-usbgadget.sh` also uncomments the two `adbd_enable` lines in the boot branch
  (idempotent like its other patches). `INIT` and `BACKUP_DIR` become overridable so it can
  be tested against a copy.
- `install.sh`: preflight (Wave on adb, binary built, `llvm-objcopy` present, enough room on
  `/data`) before anything on the Wave changes; strip; push to `/tmp` and compare md5; stop
  the stock app, the watchdog and waved, wait for waved to exit; swap; push `config.toml`
  only if the Wave has none; install `S60waved`, `S03splash`, `splash.fb`, `rsyslog.conf`,
  `iptables.rules` and the two stock stubs; run the gadget patch on the Wave; refuse to
  report success unless ADB is enabled at boot.
- `revert.sh` also removes `S03splash`, `rsyslog.conf` and `S89usbgadget` from the overlay,
  and says that ADB is off again after the reboot, as on stock.
- Tests: a host test runs both scripts against a fake `adb` (order of operations, md5
  mismatch, low space, no device, ADB-at-boot check, config preserved). The gadget patch is
  tested on a Wave against a `/tmp` copy of the stock script. One real install on a Wave
  happens with the owner's go.

## Going public

### 1. Tree scrub (normal commits on `feat/public-site`)

Replace in every tracked file, with placeholders consistent with existing fixtures
(`Office-5G`, `IW1A2B3C4D`, `aa:bb:cc:dd:ee:ff`, `192.168.1.x`):

- the home Wi-Fi SSID and its router BSSID (`waved/crates/waved-wifi/src/wpa.rs` fixture,
  `docs/REPORT_DEVICE_DUMP.md`)
- the device serial and device ID (`docs/REPORT_DEVICE_DUMP.md`, `backups/README.md`,
  `docs/superpowers/specs/2026-04-08-waved-flash-deployment-design.md`)
- the device's Wi-Fi MAC and LAN IP, and a Bluetooth address in an M3 plan

The exact values live in a replacements file **outside the repository**; this document
names only their categories. Remove `decompiled/`. Reword every link into it as prose
(for example "decompiled `libins_entry.so`, line 117173"). Tests that touch the changed
fixture still pass.

### 2. History rewrite

Preconditions: `feat/m3` is merged to `main`, the merged `worktree-agent-*` branches are
deleted, and no other session is mid-commit.

On a fresh `git clone --mirror`:

```
git filter-repo \
  --invert-paths --path decompiled/ --path firmware/ --path waved/rust_out \
  --replace-text <replacements file outside the repo> \
  --replace-message <same file>
```

(Commit messages name the device ID too, hence `--replace-message`.)

(`firmware/` holds old vendor `sw-description` and `.sig` files, already gone from the
tree. `waved/rust_out` is a stray build artefact.)

Verify on the mirror, all must be zero: `gitleaks git --log-opts=--all`, a `git log -p --all`
grep for each original value (diffs and messages), and `git log --all -- decompiled/`.

### 3. Publish

1. Rename the private repo to `insta360-custom-firmware-archive`, then immediately point
   every local checkout's `origin` at the archive URL. Once a new repo takes the old name,
   the old URL resolves to it, and a push from an old checkout would publish the
   unscrubbed history.
2. Create `insta360-custom-firmware` (public) and push the rewritten mirror.
3. Enable Pages (source: GitHub Actions) and run the workflow.
4. Re-clone locally; the old checkout and its worktrees point at the archived history.

## Testing and acceptance

- `npm test`: unit tests for link rewriting (each row of the table, anchors, `../` paths,
  directories, images) and for the missing-file error.
- `npm run build` passes with the link validator on.
- Local preview: the landing page, one synced doc with a Mermaid diagram, an NPU lab
  notebook page, and search, checked by screenshot.
- After deploy: the live URL serves the same pages.
- Scrub: the step-2 checks return zero.

## Out of scope

- Prebuilt release binaries and a download installer (decision 3).
- Notifying Insta360 about the security findings before publishing. That is the owner's call.
- Translating or restructuring the existing research documents. They are published as written.
