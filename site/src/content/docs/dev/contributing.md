---
title: Contributing
description: Branches, commit messages, docs, the tests to run, keeping personal data out, and how to report a security problem.
---

## Branches and pull requests

Branch off `main` and name the branch after the work, such as
`feat/wifi-provisioning` or `feat/public-site`. Open a pull request into
`main`. Say in it what you tested and where: on your computer only, or on a
Wave.

## Commit messages

Commits follow the conventional format, `type(scope): what changes`, with a
short subject in plain words. From the history:

```text
feat(mcu): talk to the MCU on ttyS4: power key, battery, real power off
fix(ui): list Files newest first
fix(deploy): keep the system log off /data; record the array calibration
docs: the MCU speaks MAVLink v2 on ttyS4
chore(scripts): keep the linux toolchain between container runs
test(meeting): pre-roll tests turn pre-recording on themselves
```

The type is `feat`, `fix`, `docs`, `test`, `refactor` or `chore`. The scope
is the crate or area without the `waved-` prefix (`ui`, `audio`, `mcu`,
`deploy`, `site`).

## Docs

The guides, the reference and the research on this site are the Markdown
files in `docs/` and `waved/docs/`. Edit them there; the site copies them at
build time ([Project structure](/insta360-custom-firmware/dev/structure/#how-the-site-uses-them)).

- A new document needs a line in `site/sync/manifest.mjs` (its group, path,
  page address and sidebar label) before it shows up on the site. The
  exception is `docs/npu/`: the manifest takes that whole folder, so a new
  document there is published without a line of its own.
- Link between documents with relative Markdown links. The sync step turns
  them into site links.
- The site's own pages are in `site/src/content/docs/`. Their internal links
  start with `/insta360-custom-firmware/`, and the build fails on a link or
  anchor that doesn't exist.

When you learn something about the device that the code doesn't show
(a measured offset, a magic number, a hardware quirk, a protocol detail), add
it to the matching document in `waved/docs/`. Commit messages are not where
those facts live.

## Tests to run

Run the tests for what you changed ([Testing](/insta360-custom-firmware/dev/testing/)):

- the unit tests of every crate you touched, `cargo test -p <crate> --lib`;
- for `waved-meeting` or `waved`, the same through `scripts/linux-test.sh`;
- for a screen, the UI snapshot tests, and new goldens if the screen changed
  on purpose;
- for `deploy/`, `deploy/tests/test-deploy-scripts.sh`;
- for docs or the site, `npm test` and `npm run build` in `site/`.

A change to how the hardware behaves needs a run on a Wave as well. Say which
script you ran, or what you checked by hand.

## No personal data

Never add a real Wi-Fi network name (SSID), MAC address, IP address, serial
number or device ID to fixtures, tests or docs. That includes values copied
from your own Wave's logs. Use these placeholders:

| Kind | Placeholder |
|---|---|
| Wi-Fi network name | `Office-5G` |
| MAC address | `aa:bb:cc:dd:ee:ff` |
| IP address | `192.168.1.x` |
| Serial number or device ID | `IW1A2B3C4D` |

## Security problems

Don't open a public issue for a security problem. Open a private security
advisory on the project's GitHub repository instead (its **Security** tab,
**Report a vulnerability**).
