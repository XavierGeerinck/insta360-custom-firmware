---
title: Project structure
description: What lives where in the repository, which docs belong to the firmware and which to the research, and how the site picks them up.
---

## The top level

```text
README.md        what the project is, in short
TODO.md          the backlog: everything still missing, with decisions
waved/           the firmware
docs/            reverse-engineering research on the stock firmware
site/            this site
scripts/         Ghidra decompile, firmware diff and stock-asset helpers
firmwares/       how to extract and decompile stock firmware, and notes per version
```

The stock firmware files themselves, and what is extracted and decompiled
from them, stay out of git (`.gitignore`).

## `waved/`

The firmware: a Rust workspace with one program, `waved`, and its tools.

```text
waved/
  crates/                one crate per service, plus tools and probes
  deploy/                install.sh, revert.sh, the start-up scripts S60waved and
                         S03splash, the splash image, firewall rules, rsyslog.conf
    tests/               test for install.sh and revert.sh against a fake adb
  scripts/               build and test helpers: linux-test.sh, enable_adb.py,
                         on-device acceptance tests, webhook_receiver.py
  config/default.toml    the config.toml a new install starts with
  docs/                  hardware and protocol reference, guides
  tools/                 NPU research tooling
  Cross.toml             cross's settings: the build container and its variables
  Dockerfile.cross-aarch64   the build container
  rust-toolchain.toml    stable Rust plus the aarch64 target
```

[Architecture](/insta360-custom-firmware/how/architecture/#services) says what
each crate does.

## `docs/` and `waved/docs/`

The two docs folders hold different things:

- **`waved/docs/`** describes the Wave as waved runs it: the hardware
  reference (audio, compute, display, MCU, Bluetooth, the Wave Controller
  protocol) and the guides (Wi-Fi, webhooks, updates). These facts were
  measured on a Wave or read from the stock firmware, and they are hard to
  work out again from the code. `waved/docs/ui/` holds the UI coordinate
  system and notes on the stock UI.
- **`docs/`** is the research on the stock firmware: the reverse-engineering
  reports, the security questions, the device dump, the guides for getting
  into a Wave, the analysis reports and the NPU research in `docs/npu/`.

Both also have a `superpowers/` folder with design specs and implementation
plans, and `docs/mockups/` has the design mockups for waved's screens. Those
stay in the repository and are not on the site.

## How the site uses them

The site lives in `site/`. Its own pages (Getting started, this Development
section, the features tour and How it works) are Markdown files in
`site/src/content/docs/`.

The guides, the reference and the research are not copied into `site/` in
git. `site/sync/manifest.mjs` lists each repository document that becomes a
page, with its address and its sidebar label, and `npm run sync` copies them
in before every build:

- The first `# heading` becomes the page title.
- A link to another listed document becomes a link to its page. A link to any
  other file in the repository becomes a link to that file on GitHub.
- The "Edit page" link points at the original file.

The same step copies the UI screenshots from
`waved/crates/waved-ui/golden/` (the `screens` list in the manifest).

To run the site locally (Node 22.12 or newer):

```bash
cd site
npm install
npm run dev
```
