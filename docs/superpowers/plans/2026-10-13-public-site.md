# Public Site and Going Public Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Publish the Wave open firmware: a GitHub Pages site (what it is, what it does, how it works, how to install, every doc searchable), install/revert scripts a stranger can trust, and a scrubbed public repository.

**Architecture:** Astro Starlight in `site/`. A sync script copies an allowlist of the existing Markdown (`docs/`, `waved/docs/`) into the site at build time, rewriting links; hand-written pages cover install, architecture and development. GitHub Actions builds and deploys. The repository goes public as a new repo built from a `git filter-repo` rewrite of this one.

**Tech Stack:** Node ≥ 22.12 (CI: `lts/*`), Astro 7, `@astrojs/starlight` 0.42, `astro-mermaid` 2 + `mermaid` ^11, `starlight-links-validator` 0.27, `node:test`; bash + a fake `adb` for script tests; `git filter-repo`, `gitleaks`, `actionlint`, `shellcheck`.

**Spec:** `docs/superpowers/specs/2026-10-13-public-site-design.md`

## Global Constraints

- Work happens in the worktree `.claude/worktrees/public-site` on `feat/public-site` (off `feat/m3`). Never switch branches in the main checkout: another session commits to `feat/m3` there.
- Never edit anything under `docs/npu/`. A parallel session owns it. Validator failures there are handled with an `exclude` entry in `site/astro.config.mjs`.
- Site URL `https://xaviergeerinck.github.io/insta360-custom-firmware/`; Astro `base` = `/insta360-custom-firmware`. Every internal link in site content starts with `/insta360-custom-firmware/` (the validator rejects relative links and links without the base).
- `mermaid` stays on `^11` (the `astro-mermaid` peer range). `mermaid()` comes before `starlight()` in `integrations`.
- Not on the site: `docs/superpowers/`, `waved/docs/superpowers/`, `TODO.md`, `waved/TODO.md`, `waved/CLAUDE.md`, `docs/mockups/`, crate READMEs.
- No real SSID, BSSID, MAC, IP, serial or device ID in any new file. The real values exist only in `~/.config/waved/public-scrub.txt` (outside the repo, mode 0600). Placeholders: `Office 5G`, `aa:bb:cc:dd:ee:01`, `AA:BB:CC:DD:EE:02`, `192.168.1.50`, `PSPLB0000AAAAA`, BT MAC `A1:B2:C3:D4:E5:57` (its bytes sum to the original's mod 256, so the BT frame checksum `0xFD` in `waved-bt` tests stays valid).
- The adb device ID `PSP_0000000000000000` is **kept** (chip ID, not the cloud identity; ~60 NPU scripts pin it as a device-safety check). Changing that is the owner's call.
- Hardware facts come from `waved/docs/*.md`, not the README (the README's hardware table is stale: it names a BES2600 Bluetooth chip and an "Amlogic A5"; the reference docs say Telink and A113X2).
- Every behavioural claim on a new page must be traceable to the code or an existing doc on this branch. Features that are host-tested only say so (e.g. updates: `waved/docs/UPDATES.md`, "Status").
- Commits end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Outward steps (repo rename, repo create, push, visibility, Pages) and any install on the real Wave wait for the owner's explicit go at that moment.

## Review Focus

- An old checkout pushing after the rename: once a new repo takes the old name, `origin` in every existing checkout resolves to the public repo; a push would publish the unscrubbed history. Expect `origin` re-pointed to the archive before the new repo exists (Task 14, Step 2 check).
- Re-running `install.sh` on a Wave whose `/data` is nearly full: expect a refusal before anything on the Wave is stopped or replaced, never a Wave without a binary (Task 5 test `low_space_refuses_before_touching_the_wave`).
- Rebooting after an install that could not enable ADB at boot: expect `install.sh` to exit non-zero instead of printing "Reboot" (Task 5 test `refuses_when_adb_is_not_enabled_at_boot`).
- Link-shaped text inside fenced code and inline code in a synced doc (a `[x](y)` in a bash snippet): expect it untouched (Task 2 tests `leaves_fenced_code_alone`, `leaves_inline_code_alone`).
- The parallel NPU session adding a doc with a broken link or anchor: expect a failing build whose fix is an `exclude` entry, and `git diff feat/m3 -- docs/npu` empty at the end (Task 4, Step 6; Task 12, Step 4).

---

## File structure

| Path | Responsibility |
|---|---|
| `site/package.json` | scripts `sync`, `dev`, `build`, `preview`, `test`; pinned deps |
| `site/site.config.mjs` | `SITE`, `BASE`, `REPO`, `REPO_URL`, `BRANCH`: the only place these live |
| `site/astro.config.mjs` | integrations, Starlight config, validator excludes |
| `site/src/content.config.ts` | Starlight docs collection |
| `site/sync/manifest.mjs` | allowlist of synced docs and screenshots |
| `site/sync/sidebar.mjs` | sidebar built from the manifest + hand-written pages |
| `site/scripts/lib/links.mjs` | `rewriteLinks()`: pure link rewriting |
| `site/scripts/lib/page.mjs` | `toPage()`: front matter + H1 removal |
| `site/scripts/lib/expand.mjs` | `expandEntries()`, `slugFor()`: manifest → page list |
| `site/scripts/sync-docs.mjs` | I/O runner |
| `site/scripts/test/*.test.mjs` | `node --test` suites |
| `site/src/content/docs/{index.mdx,features.md,start/,how/,dev/}` | hand-written pages (committed) |
| `site/src/content/docs/{guides,reference,research}/`, `site/src/assets/screens/`, `site/public/repo-assets/` | generated, gitignored |
| `waved/deploy/install.sh`, `revert.sh` | refreshed scripts |
| `waved/deploy/tests/test-deploy-scripts.sh` | fake-adb tests for both |
| `waved/scripts/patch-stock-usbgadget.sh` | + ADB at boot, overridable paths |
| `waved/scripts/tests/test-patch-usbgadget-on-device.sh` | runs the patch on a `/tmp` copy on a Wave |
| `.github/workflows/pages.yml` | build on PR, build + deploy on `main` |
| `README.md`, `LICENSE`, `.gitignore` | repo hygiene |

---

### Task 1: Scaffold the Starlight site

**Files:**
- Create: `site/package.json`, `site/site.config.mjs`, `site/astro.config.mjs`, `site/src/content.config.ts`, `site/src/content/docs/index.mdx` (temporary one-liner, replaced in Task 8), `site/sync/sidebar.mjs` (temporary, replaced in Task 4)
- Modify: `.gitignore`

**Interfaces:**
- Produces: `site.config.mjs` exports `SITE`, `BASE`, `REPO`, `REPO_URL`, `BRANCH` (strings). `npm run build` in `site/` produces `site/dist/`.

- [ ] **Step 1: Write `site/site.config.mjs`**

```js
// The only place the site's address and the repository live. Changing the
// repository name or moving to a custom domain is an edit here.
export const SITE = 'https://xaviergeerinck.github.io';
export const BASE = '/insta360-custom-firmware';
export const REPO = 'XavierGeerinck/insta360-custom-firmware';
export const REPO_URL = `https://github.com/${REPO}`;
export const BRANCH = 'main';
```

- [ ] **Step 2: Write `site/package.json`**

```json
{
  "name": "wave-open-firmware-site",
  "private": true,
  "type": "module",
  "engines": { "node": ">=22.12.0" },
  "scripts": {
    "sync": "node scripts/sync-docs.mjs",
    "dev": "npm run sync && astro dev",
    "build": "npm run sync && astro build",
    "preview": "astro preview",
    "test": "node --test scripts/test/"
  },
  "dependencies": {
    "@astrojs/starlight": "^0.42.6",
    "astro": "^7.3.8",
    "astro-mermaid": "^2.1.0",
    "mermaid": "^11.17.2",
    "sharp": "^0.35.3",
    "starlight-links-validator": "^0.27.0"
  }
}
```

Until Task 4 creates `scripts/sync-docs.mjs`, create it as a stub so `npm run build` works:

```js
// Replaced in Task 4.
console.log('sync: nothing to do yet');
```

- [ ] **Step 3: Write `site/src/content.config.ts`**

```ts
import { defineCollection } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
	docs: defineCollection({ loader: docsLoader(), schema: docsSchema() }),
};
```

- [ ] **Step 4: Write `site/sync/sidebar.mjs` (temporary) and `site/astro.config.mjs`**

```js
// site/sync/sidebar.mjs: replaced in Task 4.
export const sidebar = [];
```

```js
// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import mermaid from 'astro-mermaid';
import starlightLinksValidator from 'starlight-links-validator';
import { SITE, BASE, REPO_URL, BRANCH } from './site.config.mjs';
import { sidebar } from './sync/sidebar.mjs';

// Links the validator cannot check because their target is generated by a
// document we must not edit (docs/npu belongs to a parallel session). Each
// entry names the source document it comes from.
const VALIDATOR_EXCLUDES = [];

export default defineConfig({
	site: SITE,
	base: BASE,
	integrations: [
		// Must come before starlight() so it sees the Markdown first.
		mermaid(),
		starlight({
			title: 'Wave Open Firmware',
			description:
				'Open firmware for the Insta360 Wave speakerphone: no telemetry, recordings that stay yours, and a one-script way back to stock.',
			social: [{ icon: 'github', label: 'GitHub', href: REPO_URL }],
			// Hand-written pages; synced pages carry their own editUrl.
			editLink: { baseUrl: `${REPO_URL}/edit/${BRANCH}/site/` },
			lastUpdated: false,
			plugins: [starlightLinksValidator({ exclude: VALIDATOR_EXCLUDES })],
			sidebar,
		}),
	],
});
```

- [ ] **Step 5: Write a temporary `site/src/content/docs/index.mdx`**

```mdx
---
title: Wave Open Firmware
template: splash
---

Under construction.
```

- [ ] **Step 6: Ignore generated output.** Append to `.gitignore`:

```gitignore
# Public site (site/): dependencies, build output, and pages generated by
# scripts/sync-docs.mjs from docs/ and waved/docs/
site/node_modules/
site/dist/
site/.astro/
site/src/content/docs/guides/
site/src/content/docs/reference/
site/src/content/docs/research/
site/src/assets/screens/
site/public/repo-assets/
```

- [ ] **Step 7: Install and build**

Run: `cd site && npm install && npm run build`
Expected: `npm install` writes `package-lock.json`; build ends with `[build] Complete!` and `site/dist/index.html` exists. A warning that the `i18n` collection is empty is fine.

- [ ] **Step 8: Commit**

```bash
git add .gitignore site/package.json site/package-lock.json site/site.config.mjs site/astro.config.mjs site/src/content.config.ts site/src/content/docs/index.mdx site/sync/sidebar.mjs site/scripts/sync-docs.mjs
git commit -m "feat(site): scaffold the Starlight site

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Link rewriter (`rewriteLinks`)

**Files:**
- Create: `site/scripts/lib/links.mjs`
- Test: `site/scripts/test/links.test.mjs`

**Interfaces:**
- Produces:

```js
/**
 * @param {string} markdown
 * @param {{
 *   sourcePath: string,                       // repo-relative, e.g. 'waved/docs/WIFI.md'
 *   urlFor: (repoPath: string) => string | undefined,  // site URL of a synced doc
 *   isDir: (repoPath: string) => boolean,
 *   assetUrlFor: (repoPath: string) => string, // site URL for a copied image
 *   blobBase: string,                         // `${REPO_URL}/blob/${BRANCH}`
 *   treeBase: string,                         // `${REPO_URL}/tree/${BRANCH}`
 * }} ctx
 * @returns {{ markdown: string, repoLinks: string[], assets: string[] }}
 *   repoLinks: repo paths sent to GitHub; assets: repo paths of images to copy.
 */
export function rewriteLinks(markdown, ctx)
```

- [ ] **Step 1: Write the failing tests** (`site/scripts/test/links.test.mjs`)

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { rewriteLinks } from '../lib/links.mjs';

const BASE = '/insta360-custom-firmware';
const SYNCED = {
	'waved/docs/UPDATES.md': `${BASE}/guides/updates/`,
	'waved/docs/MCU_PROTOCOL.md': `${BASE}/reference/mcu-protocol/`,
	'docs/REPORT.md': `${BASE}/research/report/`,
	'docs/npu/abi/README.md': `${BASE}/research/npu/abi/`,
};
const DIRS = new Set(['docs/npu/lab-notebook', 'waved/scripts/npu']);

function ctx(sourcePath) {
	return {
		sourcePath,
		urlFor: (p) => SYNCED[p],
		isDir: (p) => DIRS.has(p),
		assetUrlFor: (p) => `${BASE}/repo-assets/${p}`,
		blobBase: 'https://github.com/o/r/blob/main',
		treeBase: 'https://github.com/o/r/tree/main',
	};
}
const run = (md, src = 'waved/docs/WIFI.md') => rewriteLinks(md, ctx(src));

test('synced_doc_becomes_site_url', () => {
	assert.equal(run('See [updates](UPDATES.md).').markdown, `See [updates](${BASE}/guides/updates/).`);
});

test('anchor_is_kept', () => {
	assert.equal(
		run('[f](MCU_PROTOCOL.md#frame-format)').markdown,
		`[f](${BASE}/reference/mcu-protocol/#frame-format)`,
	);
});

test('parent_paths_resolve_to_github_blob', () => {
	const r = run('[py](../../../waved/scripts/npu/nbg_format.py)', 'docs/npu/abi/README.md');
	assert.equal(r.markdown, '[py](https://github.com/o/r/blob/main/waved/scripts/npu/nbg_format.py)');
	assert.deepEqual(r.repoLinks, ['waved/scripts/npu/nbg_format.py']);
});

test('directory_uses_tree', () => {
	assert.equal(
		run('[nb](../lab-notebook/)', 'docs/npu/abi/README.md').markdown,
		'[nb](https://github.com/o/r/tree/main/docs/npu/lab-notebook)',
	);
});

test('decompiled_becomes_plain_text', () => {
	assert.equal(run('From [`pspapp.c`](../../decompiled/pspapp.c) line 9.').markdown, 'From `pspapp.c` line 9.');
});

test('root_absolute_path_is_repo_root', () => {
	assert.equal(run('[r](/docs/REPORT.md)').markdown, `[r](${BASE}/research/report/)`);
});

test('external_mailto_and_hash_unchanged', () => {
	const md = '[a](https://x.org/a_(b)) [m](mailto:a@b.c) [h](#here)';
	assert.equal(run(md).markdown, md);
});

test('title_is_kept', () => {
	assert.equal(run('[u](UPDATES.md "Updates")').markdown, `[u](${BASE}/guides/updates/ "Updates")`);
});

test('nested_brackets_in_label', () => {
	assert.equal(run('[buf[0] value](UPDATES.md)').markdown, `[buf[0] value](${BASE}/guides/updates/)`);
});

test('outside_repo_becomes_plain_text', () => {
	assert.equal(run('[x](../../../../etc/passwd)').markdown, 'x');
});

test('image_is_copied_as_asset', () => {
	const r = run('![dial](img/dial.png)');
	assert.equal(r.markdown, `![dial](${BASE}/repo-assets/waved/docs/img/dial.png)`);
	assert.deepEqual(r.assets, ['waved/docs/img/dial.png']);
});

test('leaves_fenced_code_alone', () => {
	const md = '```bash\n[x](UPDATES.md)\n```\n~~~\n[y](UPDATES.md)\n~~~';
	assert.equal(run(md).markdown, md);
});

test('leaves_inline_code_alone', () => {
	const md = 'Run `echo [x](UPDATES.md)` now.';
	assert.equal(run(md).markdown, md);
});

test('percent_encoded_path', () => {
	const r = run('[s](../../waved/scripts/npu/my%20file.py)');
	assert.equal(r.markdown, '[s](https://github.com/o/r/blob/main/waved/scripts/npu/my%20file.py)');
	assert.deepEqual(r.repoLinks, ['waved/scripts/npu/my file.py']);
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd site && node --test scripts/test/links.test.mjs`
Expected: FAIL, `Cannot find module '../lib/links.mjs'`.

- [ ] **Step 3: Implement `site/scripts/lib/links.mjs`**

```js
import path from 'node:path';

// A scheme (http:, mailto:, …) or a protocol-relative URL.
const EXTERNAL = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;
const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
// [label](target "title") and ![alt](target). Labels may hold one level of
// nested brackets; targets may be wrapped in <>.
const LINK = /(!?)\[((?:[^[\]]|\[[^[\]]*\])*)\]\(\s*<?([^)\s>]+)>?((?:\s+"[^"]*")?)\s*\)/g;
const CODE_SPAN = /(`+)(?:(?!\1)[\s\S])*?\1/g;

export function rewriteLinks(markdown, ctx) {
	const repoLinks = [];
	const assets = [];
	let fence = null;
	const lines = markdown.split('\n').map((line) => {
		const f = line.match(FENCE);
		if (fence) {
			if (f && f[1][0] === fence[0] && f[1].length >= fence.length) fence = null;
			return line;
		}
		if (f) {
			fence = f[1];
			return line;
		}
		return rewriteLine(line, ctx, repoLinks, assets);
	});
	return { markdown: lines.join('\n'), repoLinks, assets };
}

function rewriteLine(line, ctx, repoLinks, assets) {
	const code = [];
	for (const m of line.matchAll(CODE_SPAN)) code.push([m.index, m.index + m[0].length]);
	const inCode = (i) => code.some(([a, b]) => i >= a && i < b);

	return line.replace(LINK, (whole, bang, label, target, title, offset) => {
		if (inCode(offset)) return whole;
		const r = resolve(target, ctx);
		if (r.kind === 'keep') return whole;
		if (r.kind === 'text') return label;
		if (bang) {
			assets.push(r.repoPath);
			return `![${label}](${ctx.assetUrlFor(r.repoPath)}${title})`;
		}
		if (r.kind === 'repo') repoLinks.push(r.repoPath);
		return `[${label}](${r.url}${title})`;
	});
}

function resolve(target, ctx) {
	if (EXTERNAL.test(target) || target.startsWith('#')) return { kind: 'keep' };
	const hashAt = target.indexOf('#');
	const rawPath = hashAt === -1 ? target : target.slice(0, hashAt);
	const hash = hashAt === -1 ? '' : target.slice(hashAt);
	let decoded = rawPath;
	try {
		decoded = decodeURI(rawPath);
	} catch {}
	const joined = decoded.startsWith('/')
		? decoded.slice(1)
		: path.posix.join(path.posix.dirname(ctx.sourcePath), decoded);
	const repoPath = path.posix.normalize(joined).replace(/\/$/, '');
	if (repoPath === '..' || repoPath.startsWith('../')) return { kind: 'text' };
	if (repoPath === 'decompiled' || repoPath.startsWith('decompiled/')) return { kind: 'text' };

	const synced = ctx.urlFor(repoPath);
	if (synced) return { kind: 'site', url: synced + hash, repoPath };
	const base = ctx.isDir(repoPath) ? ctx.treeBase : ctx.blobBase;
	return { kind: 'repo', url: `${base}/${encodeURI(repoPath)}${hash}`, repoPath };
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd site && node --test scripts/test/links.test.mjs`
Expected: PASS, 14 tests, 0 failures.

- [ ] **Step 5: Commit**

```bash
git add site/scripts/lib/links.mjs site/scripts/test/links.test.mjs
git commit -m "feat(site): rewrite links in synced docs

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Page transform and manifest expansion

**Files:**
- Create: `site/scripts/lib/page.mjs`, `site/scripts/lib/expand.mjs`
- Test: `site/scripts/test/page.test.mjs`, `site/scripts/test/expand.test.mjs`

**Interfaces:**
- Produces:
  - `toPage(markdown: string, { sourcePath: string, editUrl: string }) => string`: Markdown with Starlight front matter (`title`, `editUrl`) and the first H1 removed.
  - `slugFor(globRoot: string, slugPrefix: string, repoPath: string) => { slug: string, isIndex: boolean }`
  - `expandEntries(entries: Entry[], repoRoot: string) => Page[]` where `Entry = { group, source, slug, label } | { group, glob, slugPrefix, exclude?: string[] }` and `Page = { group, source, slug, label?, isIndex, outFile }` (`outFile` relative to `src/content/docs/`). Throws `Error` naming the path when an allowlisted `source` or `glob` root does not exist.

- [ ] **Step 1: Write the failing tests**

`site/scripts/test/page.test.mjs`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { toPage } from '../lib/page.mjs';

const opts = { sourcePath: 'waved/docs/MCU_PROTOCOL.md', editUrl: 'https://e/MCU_PROTOCOL.md' };

test('h1_becomes_title_and_is_removed', () => {
	assert.equal(
		toPage('# MCU Protocol\n\nBody.\n', opts),
		'---\ntitle: "MCU Protocol"\neditUrl: "https://e/MCU_PROTOCOL.md"\n---\n\nBody.\n',
	);
});

test('code_and_links_are_stripped_from_title', () => {
	const out = toPage('# MCU — `/dev/ttyS4`, [MAVLink](https://mavlink.io) v2\n', opts);
	assert.match(out, /^---\ntitle: "MCU — \/dev\/ttyS4, MAVLink v2"\n/);
});

test('quotes_in_title_are_escaped', () => {
	assert.match(toPage('# The "PC Link" report\n', opts), /title: "The \\"PC Link\\" report"/);
});

test('h1_inside_code_fence_is_not_the_title', () => {
	const out = toPage('```bash\n# not a title\n```\n\n# Real title\n', opts);
	assert.match(out, /title: "Real title"/);
	assert.match(out, /# not a title/);
});

test('no_h1_uses_file_name', () => {
	assert.match(toPage('Text only.\n', { ...opts, sourcePath: 'docs/npu/trace/galcore-interface.md' }), /title: "galcore interface"/);
});

test('existing_front_matter_is_kept', () => {
	const out = toPage('---\nsidebar:\n  order: 2\n---\n# T\n\nB\n', opts);
	assert.equal(out, '---\ntitle: "T"\neditUrl: "https://e/MCU_PROTOCOL.md"\nsidebar:\n  order: 2\n---\n\nB\n');
});
```

`site/scripts/test/expand.test.mjs`:

```js
import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { expandEntries, slugFor } from '../lib/expand.mjs';

test('slug_for_readme_is_directory_index', () => {
	assert.deepEqual(slugFor('docs/npu', 'research/npu', 'docs/npu/README.md'), { slug: 'research/npu', isIndex: true });
	assert.deepEqual(slugFor('docs/npu', 'research/npu', 'docs/npu/abi/README.md'), { slug: 'research/npu/abi', isIndex: true });
});

test('slug_is_lowercase_kebab', () => {
	assert.deepEqual(
		slugFor('docs/npu', 'research/npu', 'docs/npu/lab-notebook/2026-07-28-PoC0_Lab.md'),
		{ slug: 'research/npu/lab-notebook/2026-07-28-poc0-lab', isIndex: false },
	);
});

function tmpRepo(files) {
	const root = fs.mkdtempSync(path.join(os.tmpdir(), 'sync-'));
	for (const f of files) {
		fs.mkdirSync(path.join(root, path.dirname(f)), { recursive: true });
		fs.writeFileSync(path.join(root, f), '# x\n');
	}
	return root;
}

test('glob_expands_sorted_and_honours_exclude', () => {
	const root = tmpRepo(['docs/npu/README.md', 'docs/npu/b.md', 'docs/npu/lab/000-template.md', 'docs/npu/lab/a.md', 'docs/npu/x.json']);
	const pages = expandEntries([{ group: 'npu', glob: 'docs/npu', slugPrefix: 'research/npu', exclude: ['lab/000-template.md'] }], root);
	assert.deepEqual(pages.map((p) => [p.source, p.outFile]), [
		['docs/npu/README.md', 'research/npu/index.md'],
		['docs/npu/b.md', 'research/npu/b.md'],
		['docs/npu/lab/a.md', 'research/npu/lab/a.md'],
	]);
});

test('missing_source_throws_with_its_path', () => {
	const root = tmpRepo([]);
	assert.throws(
		() => expandEntries([{ group: 'g', source: 'waved/docs/GONE.md', slug: 'reference/gone', label: 'Gone' }], root),
		/waved\/docs\/GONE\.md/,
	);
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd site && node --test scripts/test/`
Expected: FAIL for `page.test.mjs` and `expand.test.mjs` (`Cannot find module`); `links.test.mjs` still passes.

- [ ] **Step 3: Implement `site/scripts/lib/page.mjs`**

```js
import path from 'node:path';

const FENCE = /^\s{0,3}(`{3,}|~{3,})/;
const H1 = /^#\s+(.+?)\s*#*\s*$/;

export function toPage(markdown, { sourcePath, editUrl }) {
	let body = markdown;
	let existing = [];
	const fm = body.match(/^---\n([\s\S]*?)\n---\n/);
	if (fm) {
		existing = fm[1].split('\n');
		body = body.slice(fm[0].length);
	}

	const lines = body.split('\n');
	let fence = null;
	let title = null;
	for (let i = 0; i < lines.length; i++) {
		const f = lines[i].match(FENCE);
		if (fence) {
			if (f && f[1][0] === fence[0] && f[1].length >= fence.length) fence = null;
			continue;
		}
		if (f) {
			fence = f[1];
			continue;
		}
		const h = lines[i].match(H1);
		if (h) {
			title = cleanTitle(h[1]);
			lines.splice(i, lines[i + 1] === '' ? 2 : 1);
			break;
		}
	}
	if (title === null) title = path.basename(sourcePath, path.extname(sourcePath)).replace(/[-_]+/g, ' ');

	const front = [`title: ${JSON.stringify(title)}`, `editUrl: ${JSON.stringify(editUrl)}`, ...existing.filter((l) => !/^(title|editUrl):/.test(l))];
	return `---\n${front.join('\n')}\n---\n\n${lines.join('\n').replace(/^\n+/, '')}`;
}

function cleanTitle(raw) {
	return raw
		.replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
		.replace(/`/g, '')
		.replace(/\*\*|__/g, '')
		.trim();
}
```

- [ ] **Step 4: Implement `site/scripts/lib/expand.mjs`**

```js
import fs from 'node:fs';
import path from 'node:path';

export function slugFor(globRoot, slugPrefix, repoPath) {
	const rel = path.posix.relative(globRoot, repoPath).replace(/\.md$/i, '');
	const parts = rel.split('/');
	const isIndex = parts[parts.length - 1].toLowerCase() === 'readme';
	if (isIndex) parts.pop();
	const kebab = parts.map((p) => p.toLowerCase().replace(/[_\s]+/g, '-'));
	return { slug: [slugPrefix, ...kebab].join('/'), isIndex };
}

export function expandEntries(entries, repoRoot) {
	const pages = [];
	for (const e of entries) {
		if (e.source) {
			mustExist(repoRoot, e.source);
			pages.push({ ...e, isIndex: false, outFile: `${e.slug}.md` });
			continue;
		}
		mustExist(repoRoot, e.glob);
		const exclude = new Set((e.exclude ?? []).map((x) => path.posix.join(e.glob, x)));
		for (const source of walk(repoRoot, e.glob)) {
			if (exclude.has(source)) continue;
			const { slug, isIndex } = slugFor(e.glob, e.slugPrefix, source);
			pages.push({ group: e.group, source, slug, isIndex, outFile: isIndex ? `${slug}/index.md` : `${slug}.md` });
		}
	}
	return pages;
}

function mustExist(repoRoot, p) {
	if (!fs.existsSync(path.join(repoRoot, p))) {
		throw new Error(`sync manifest names ${p}, which does not exist. Update site/sync/manifest.mjs.`);
	}
}

function walk(repoRoot, dir) {
	const out = [];
	for (const d of fs.readdirSync(path.join(repoRoot, dir), { withFileTypes: true })) {
		const p = path.posix.join(dir, d.name);
		if (d.isDirectory()) out.push(...walk(repoRoot, p));
		else if (d.name.toLowerCase().endsWith('.md')) out.push(p);
	}
	// README first in each directory, then by name.
	return out.sort((a, b) => key(a).localeCompare(key(b)));
}

const key = (p) => p.replace(/README\.md$/i, '');
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd site && npm test`
Expected: PASS, all suites (links 14, page 6, expand 4), 0 failures.

- [ ] **Step 6: Commit**

```bash
git add site/scripts/lib/page.mjs site/scripts/lib/expand.mjs site/scripts/test/page.test.mjs site/scripts/test/expand.test.mjs
git commit -m "feat(site): page front matter and manifest expansion

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Manifest, sidebar, sync runner, full build

**Files:**
- Create: `site/sync/manifest.mjs`
- Modify: `site/sync/sidebar.mjs` (replace), `site/scripts/sync-docs.mjs` (replace stub), `site/astro.config.mjs` (`VALIDATOR_EXCLUDES` only, if needed)
- Modify (only if the validator finds broken anchors in them): files under `docs/` other than `docs/npu/`, and `waved/docs/`

**Interfaces:**
- Consumes: `rewriteLinks` (Task 2), `toPage`, `expandEntries` (Task 3), `BASE`, `REPO_URL`, `BRANCH` (Task 1).
- Produces: `manifest.mjs` exports `entries` (array of `Entry`) and `screens` (array of golden PNG base names). Generated pages under `src/content/docs/{guides,reference,research}/`; screenshots in `src/assets/screens/<name>.png`. Later tasks link to these slugs.

- [ ] **Step 1: Write `site/sync/manifest.mjs`**

```js
// Which repository documents become site pages. An explicit list on purpose:
// nothing internal can reach the public site through a stray glob. A listed
// file that no longer exists fails the build (scripts/lib/expand.mjs).
export const entries = [
	// Guides
	{ group: 'guides', source: 'waved/docs/WIFI.md', slug: 'guides/wifi', label: 'Wi-Fi & provisioning' },
	{ group: 'guides', source: 'waved/docs/WEBHOOKS.md', slug: 'guides/webhooks', label: 'Recording webhooks' },
	{ group: 'guides', source: 'waved/docs/UPDATES.md', slug: 'guides/updates', label: 'Updates' },

	// Hardware & protocol reference
	{ group: 'reference', source: 'waved/docs/AUDIO_HARDWARE.md', slug: 'reference/audio-hardware', label: 'Audio hardware' },
	{ group: 'reference', source: 'waved/docs/COMPUTE_HARDWARE.md', slug: 'reference/compute-hardware', label: 'Compute hardware' },
	{ group: 'reference', source: 'waved/docs/DISPLAY_HARDWARE.md', slug: 'reference/display-hardware', label: 'Display' },
	{ group: 'reference', source: 'waved/docs/MCU_PROTOCOL.md', slug: 'reference/mcu-protocol', label: 'MCU protocol' },
	{ group: 'reference', source: 'waved/docs/BLUETOOTH_PROTOCOL.md', slug: 'reference/bluetooth-protocol', label: 'Bluetooth protocol' },
	{ group: 'reference', source: 'waved/docs/WAVE_CONTROLLER_PROTOCOL.md', slug: 'reference/wave-controller-protocol', label: 'Wave Controller protocol' },
	{ group: 'reference', source: 'waved/docs/ui/COORDINATE_SYSTEM.md', slug: 'reference/ui-coordinates', label: 'UI coordinate system' },

	// Research
	{ group: 'research', source: 'docs/REPORT.md', slug: 'research/report', label: 'Reverse-engineering report' },
	{ group: 'research', source: 'docs/QUESTIONS_SECURITY.md', slug: 'research/security', label: 'Security Q&A' },
	{ group: 'research', source: 'docs/REPORT_DEVICE_DUMP.md', slug: 'research/device-dump', label: 'Device dump' },
	{ group: 'research', source: 'docs/REPORT_AI_MODELS.md', slug: 'research/ai-models', label: 'AI models' },
	{ group: 'research', source: 'docs/REPORT_NPU.md', slug: 'research/npu-report', label: 'NPU report' },
	{ group: 'research', source: 'docs/REPORT_UI.md', slug: 'research/ui-report', label: 'Stock UI report' },
	{ group: 'research', source: 'docs/FIRMWARE_GAP_ANALYSIS.md', slug: 'research/gap-analysis', label: 'Firmware gap analysis' },
	{ group: 'research', source: 'docs/DEBUG_LATENCY.md', slug: 'research/debug-latency', label: 'Debugging latency' },
	{ group: 'research', source: 'docs/DEBUG_PIPELINE.md', slug: 'research/debug-pipeline', label: 'Debugging the audio pipeline' },
	{ group: 'research-access', source: 'docs/GUIDE_DEVICE_ACCESS.md', slug: 'research/device-access', label: 'Device access' },
	{ group: 'research-access', source: 'docs/GUIDE_EXTRACT_MODELS.md', slug: 'research/extract-models', label: 'Extracting models' },
	{ group: 'research-access', source: 'docs/GUIDE_MODEL_PARTITION_RECOVERY.md', slug: 'research/model-partition-recovery', label: 'Model partition recovery' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_Command_Protocol.md', slug: 'research/analysis/cloud-command-protocol', label: 'Cloud command protocol' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_Protocol_Decompiled_Analysis.md', slug: 'research/analysis/cloud-protocol', label: 'Cloud protocol' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Cloud_and_Audio_Library_Analysis.md', slug: 'research/analysis/cloud-and-audio-libraries', label: 'Cloud & audio libraries' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/Init_Scripts_and_Config_Analysis.md', slug: 'research/analysis/init-scripts', label: 'Init scripts & config' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/MQTT_Topic_Map.md', slug: 'research/analysis/mqtt-topics', label: 'MQTT topic map' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/NPU_Stack_Inventory.md', slug: 'research/analysis/npu-stack', label: 'NPU stack inventory' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/UI_and_Database_Decompiled_Analysis.md', slug: 'research/analysis/ui-and-database', label: 'UI & database' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/libins_entry_Binary_Analysis.md', slug: 'research/analysis/libins-entry-binary', label: 'libins_entry: binary' },
	{ group: 'research-analysis', source: 'docs/analysis_reports/libins_entry_Decompiled_Analysis.md', slug: 'research/analysis/libins-entry-decompiled', label: 'libins_entry: decompiled' },
	{ group: 'research-stock-ui', source: 'waved/docs/ui/ASSETS.md', slug: 'research/stock-ui/assets', label: 'Stock UI assets' },
	{ group: 'research-stock-ui', source: 'waved/docs/ui/HOME_SCREEN.md', slug: 'research/stock-ui/home-screen', label: 'Stock home screen' },
	{ group: 'research-npu', glob: 'docs/npu', slugPrefix: 'research/npu', exclude: ['lab-notebook/000-template.md'] },
];

// Screens rendered by the UI code itself (waved-ui snapshot tests), copied to
// src/assets/screens/<name>.png for the landing page and the features tour.
export const screens = [
	'home', 'home_muted', 'dial_live_talker', 'recording', 'recording_hold_stop', 'files', 'file_detail',
	'screen_menu', 'screen_audio_mode', 'screen_bluetooth', 'screen_call_active', 'screen_pin',
	'screen_mic_height', 'screen_uploads', 'update_installing', 'wifi_qr',
];
```

- [ ] **Step 2: Replace `site/sync/sidebar.mjs`**

```js
import { entries } from './manifest.mjs';

const group = (name) =>
	entries.filter((e) => e.group === name && e.source).map((e) => ({ label: e.label, slug: e.slug }));

export const sidebar = [
	{
		label: 'Getting started',
		items: ['start/overview', 'start/requirements', 'start/install', 'start/first-setup', 'start/revert', 'start/troubleshooting'],
	},
	{ label: 'Guides', items: ['features', ...group('guides')] },
	{ label: 'How it works', items: ['how/architecture', 'how/hardware'] },
	{ label: 'Hardware & protocol reference', items: group('reference') },
	{
		label: 'Research',
		items: [
			...group('research'),
			{ label: 'Device access & extraction', collapsed: true, items: group('research-access') },
			{ label: 'Analysis reports', collapsed: true, items: group('research-analysis') },
			{ label: 'Stock UI', collapsed: true, items: group('research-stock-ui') },
			{ label: 'NPU research', collapsed: true, autogenerate: { directory: 'research/npu' } },
		],
	},
	{ label: 'Development', items: ['dev/building', 'dev/testing', 'dev/structure', 'dev/contributing'] },
];
```

Until Tasks 6–8 add the hand-written pages, the build fails on their missing slugs. Create each as a stub now so the build can run; Tasks 6–8 overwrite them. One stub per slug (`start/overview`, `start/requirements`, `start/install`, `start/first-setup`, `start/revert`, `start/troubleshooting`, `features`, `how/architecture`, `how/hardware`, `dev/building`, `dev/testing`, `dev/structure`, `dev/contributing`), each:

```md
---
title: Coming soon
---

This page is being written.
```

- [ ] **Step 3: Replace `site/scripts/sync-docs.mjs`**

```js
// Copies the documents listed in sync/manifest.mjs into the site, rewriting
// their links, and copies the UI screenshots. Runs before `astro dev` and
// `astro build`. Its output is gitignored: the repository's Markdown stays
// the only copy.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE, REPO_URL, BRANCH } from '../site.config.mjs';
import { entries, screens } from '../sync/manifest.mjs';
import { expandEntries } from './lib/expand.mjs';
import { rewriteLinks } from './lib/links.mjs';
import { toPage } from './lib/page.mjs';

const SITE_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO_ROOT = path.resolve(SITE_DIR, '..');
const CONTENT = path.join(SITE_DIR, 'src/content/docs');
const ASSETS = path.join(SITE_DIR, 'public/repo-assets');
const SCREENS_OUT = path.join(SITE_DIR, 'src/assets/screens');
const SCREENS_IN = path.join(REPO_ROOT, 'waved/crates/waved-ui/golden');

for (const dir of ['guides', 'reference', 'research']) fs.rmSync(path.join(CONTENT, dir), { recursive: true, force: true });
fs.rmSync(ASSETS, { recursive: true, force: true });
fs.rmSync(SCREENS_OUT, { recursive: true, force: true });

const pages = expandEntries(entries, REPO_ROOT);
const urlBySource = new Map(pages.map((p) => [p.source, `${BASE}/${p.slug}/`]));
const warnings = [];

for (const p of pages) {
	const { markdown, repoLinks, assets } = rewriteLinks(fs.readFileSync(path.join(REPO_ROOT, p.source), 'utf8'), {
		sourcePath: p.source,
		urlFor: (rp) => urlBySource.get(rp),
		isDir: (rp) => fs.statSync(path.join(REPO_ROOT, rp), { throwIfNoEntry: false })?.isDirectory() ?? false,
		assetUrlFor: (rp) => `${BASE}/repo-assets/${rp}`,
		blobBase: `${REPO_URL}/blob/${BRANCH}`,
		treeBase: `${REPO_URL}/tree/${BRANCH}`,
	});
	for (const rp of repoLinks) {
		if (!fs.existsSync(path.join(REPO_ROOT, rp))) warnings.push(`${p.source}: links to ${rp}, which is not in the repository`);
	}
	for (const rp of assets) {
		const from = path.join(REPO_ROOT, rp);
		if (!fs.existsSync(from)) {
			warnings.push(`${p.source}: image ${rp} is not in the repository`);
			continue;
		}
		fs.mkdirSync(path.dirname(path.join(ASSETS, rp)), { recursive: true });
		fs.copyFileSync(from, path.join(ASSETS, rp));
	}
	const out = path.join(CONTENT, p.outFile);
	fs.mkdirSync(path.dirname(out), { recursive: true });
	fs.writeFileSync(out, toPage(markdown, { sourcePath: p.source, editUrl: `${REPO_URL}/edit/${BRANCH}/${p.source}` }));
}

fs.mkdirSync(SCREENS_OUT, { recursive: true });
for (const name of screens) {
	const from = path.join(SCREENS_IN, `${name}.png`);
	if (!fs.existsSync(from)) throw new Error(`sync manifest names screen ${name}, but ${from} does not exist`);
	fs.copyFileSync(from, path.join(SCREENS_OUT, `${name}.png`));
}

console.log(`sync: ${pages.length} docs, ${screens.length} screens`);
for (const w of warnings) console.warn(`sync: warning: ${w}`);
```

- [ ] **Step 4: Run sync and look at the output**

Run: `cd site && npm run sync && find src/content/docs -name '*.md' | wc -l && head -5 src/content/docs/reference/mcu-protocol.md`
Expected: `sync: N docs, 16 screens` with N ≈ 85; the MCU page starts with `---`, `title: "MCU Protocol — /dev/ttyS4, MAVLink v2"`, an `editUrl` ending `waved/docs/MCU_PROTOCOL.md`, `---`. Warnings about links to files not in the repository are listed; note them for Step 6.

- [ ] **Step 5: Build**

Run: `cd site && npm run build 2>&1 | tail -40`
Expected: either `[build] Complete!`, or a `starlight-links-validator` report listing invalid links/hashes per file.

- [ ] **Step 6: Fix what the validator reports**

For each reported link:
- **In a source under `docs/` (not `docs/npu/`) or `waved/docs/`:** open the source document and fix the link or anchor there (the generated page is overwritten on every sync). Anchors follow GitHub's slug rules, so a broken one is broken on GitHub too.
- **In a page generated from `docs/npu/`:** do not edit the source. Add the exact link to `VALIDATOR_EXCLUDES` in `site/astro.config.mjs` with a comment naming the source file, e.g.

```js
const VALIDATOR_EXCLUDES = [
	// docs/npu/README.md links an anchor its target no longer has.
	'/insta360-custom-firmware/research/npu/abi/backend-map/#firmware-v3-audio-api',
];
```

- **A synced doc that fails to parse** (the build stops with a Markdown/HTML error): fix the source if it is outside `docs/npu/`; otherwise add it to the manifest's `exclude` for the NPU glob with a comment and tell the owner.

Re-run `npm run build` until it prints `[build] Complete!`. Then run `npm test` (still green) and `git diff --stat feat/m3 -- docs/npu` (must print nothing).

- [ ] **Step 7: Commit**

```bash
git add site/sync/manifest.mjs site/sync/sidebar.mjs site/scripts/sync-docs.mjs site/astro.config.mjs site/src/content/docs/start site/src/content/docs/how site/src/content/docs/dev site/src/content/docs/features.md
git add -u docs waved/docs   # only if Step 6 fixed sources
git commit -m "feat(site): sync the repository docs into the site

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Install and revert scripts a stranger can trust

**Files:**
- Modify: `waved/scripts/patch-stock-usbgadget.sh`, `waved/deploy/install.sh` (rewrite), `waved/deploy/revert.sh` (rewrite)
- Create: `waved/deploy/tests/test-deploy-scripts.sh`, `waved/scripts/tests/test-patch-usbgadget-on-device.sh`

**Interfaces:**
- Produces: `install.sh` honours `WAVED_BINARY` (path to the built binary) and `OBJCOPY` (strip tool) for tests; exits non-zero with `ERROR: …` on every refusal. `patch-stock-usbgadget.sh` honours `INIT` and `BACKUP_DIR`. The Install page (Task 6) documents these scripts' behaviour.

- [ ] **Step 1: Write the failing host test** (`waved/deploy/tests/test-deploy-scripts.sh`)

```bash
#!/usr/bin/env bash
# Runs deploy/install.sh and deploy/revert.sh against a fake adb that logs
# every call and answers the few questions the scripts ask. No Wave needed.
set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
DEPLOY="$(cd "$HERE/.." && pwd)"
FAILS=0

setup() {
    T="$(mktemp -d)"
    mkdir -p "$T/bin" "$T/pushed"
    export FAKE_ADB_LOG="$T/adb.log" FAKE_ADB_DIR="$T/pushed"
    export FAKE_STATE=device FAKE_FREE_KB=177876 FAKE_CONFIG_EXISTS=no FAKE_COMMENTED_ADBD=0 FAKE_MD5=
    : > "$FAKE_ADB_LOG"
    cat > "$T/bin/adb" <<'ADB'
#!/usr/bin/env bash
echo "$*" >> "$FAKE_ADB_LOG"
case "$1" in
  get-state) echo "$FAKE_STATE" ;;
  push) cp "$2" "$FAKE_ADB_DIR/$(echo "$3" | tr / _)"; echo "1 file pushed" ;;
  shell)
    shift; cmd="$*"
    case "$cmd" in
      *"df -k /data"*) echo "/dev/data 450360 237580 $FAKE_FREE_KB 57% /data" ;;
      *"du -k /data/waved/waved"*) echo 0 ;;
      *"md5sum /tmp/waved.new"*)
        if [ -n "$FAKE_MD5" ]; then echo "$FAKE_MD5  /tmp/waved.new"
        else f="$FAKE_ADB_DIR/_tmp_waved.new"; (md5sum "$f" 2>/dev/null || md5 -r "$f") | awk '{print $1"  /tmp/waved.new"}'; fi ;;
      *"config.toml ] && echo yes"*) echo "$FAKE_CONFIG_EXISTS" ;;
      *"adbd_enable"*) echo "$FAKE_COMMENTED_ADBD" ;;
      *"cp /tmp/waved.new /data/waved/waved"*) echo ok ;;
    esac ;;
esac
ADB
    chmod +x "$T/bin/adb"
    printf '#!/bin/sh\ncp "$2" "$3"\n' > "$T/bin/objcopy"; chmod +x "$T/bin/objcopy"
    head -c 4096 /dev/urandom > "$T/waved"
    export PATH="$T/bin:$PATH" OBJCOPY="$T/bin/objcopy" WAVED_BINARY="$T/waved"
}

check() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; FAILS=$((FAILS+1)); fi; }
line_of() { grep -n -F -- "$1" "$FAKE_ADB_LOG" | head -1 | cut -d: -f1; }

# install: happy path
setup
"$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check installs_and_succeeds '[ $rc -eq 0 ]'
check pushes_to_tmp_before_swapping 'grep -n -F "/tmp/waved.new" "$FAKE_ADB_LOG" | head -1 | grep -q ":push " && [ "$(line_of "/tmp/waved.new")" -lt "$(line_of "cp /tmp/waved.new /data/waved/waved")" ]'
check stops_stock_before_swapping '[ "$(line_of "killall pspapp")" -lt "$(line_of "cp /tmp/waved.new /data/waved/waved")" ]'
for f in S60waved S03splash splash.fb rsyslog.conf iptables.rules; do
    check "pushes_$f" 'grep -q -- "$f" "$FAKE_ADB_LOG"'
done
check pushes_default_config_when_wave_has_none 'grep -q "default.toml /data/waved/config.toml" "$FAKE_ADB_LOG"'
check runs_usb_gadget_patch 'grep -q "sh /tmp/patch-usbgadget.sh" "$FAKE_ADB_LOG"'
check says_reboot 'grep -q "adb reboot" "$T/out"'

# install: keeps an existing config
setup; FAKE_CONFIG_EXISTS=yes "$DEPLOY/install.sh" > /dev/null 2>&1
check keeps_existing_config '! grep -q "default.toml" "$FAKE_ADB_LOG"'

# install: refusals
setup; FAKE_MD5=deadbeef "$DEPLOY/install.sh" > /dev/null 2>&1; rc=$?
check md5_mismatch_refuses '[ $rc -ne 0 ] && ! grep -q "cp /tmp/waved.new /data/waved/waved" "$FAKE_ADB_LOG"'

setup; FAKE_FREE_KB=1000 "$DEPLOY/install.sh" > /dev/null 2>&1; rc=$?
check low_space_refuses_before_touching_the_wave '[ $rc -ne 0 ] && ! grep -q -e "^push" -e "killall" "$FAKE_ADB_LOG"'

setup; FAKE_STATE=unknown "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check no_device_refuses '[ $rc -ne 0 ] && grep -q "adb" "$T/out"'

setup; FAKE_COMMENTED_ADBD=2 "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check refuses_when_adb_is_not_enabled_at_boot '[ $rc -ne 0 ] && ! grep -q "adb reboot" "$T/out"'

setup; WAVED_BINARY="$T/missing" "$DEPLOY/install.sh" > "$T/out" 2>&1; rc=$?
check missing_binary_explains_build '[ $rc -ne 0 ] && grep -q "cross build" "$T/out"'

# revert
setup; "$DEPLOY/revert.sh" > "$T/out" 2>&1; rc=$?
check revert_succeeds '[ $rc -eq 0 ]'
for f in S60waved S60pspapp_daemon S03splash S89usbgadget rsyslog.conf libapplogrs.so "rm -rf /data/waved"; do
    check "revert_removes_$f" 'grep -q -- "$f" "$FAKE_ADB_LOG"'
done
setup; FAKE_STATE=unknown "$DEPLOY/revert.sh" > /dev/null 2>&1; rc=$?
check revert_refuses_without_device '[ $rc -ne 0 ] && ! grep -q "rm " "$FAKE_ADB_LOG"'

echo; [ "$FAILS" -eq 0 ] && echo "all passed" || { echo "$FAILS failed"; exit 1; }
```

`chmod +x waved/deploy/tests/test-deploy-scripts.sh`

- [ ] **Step 2: Run it to verify it fails**

Run: `waved/deploy/tests/test-deploy-scripts.sh`
Expected: several `FAIL` lines (the old `install.sh` ignores `WAVED_BINARY`, pushes straight to `/data/waved/waved`, skips `S03splash`/`rsyslog.conf`/the gadget patch; the old `revert.sh` keeps `S03splash`, `S89usbgadget`, `rsyslog.conf`), ending `N failed`.

- [ ] **Step 3: Add the ADB-at-boot patch to `waved/scripts/patch-stock-usbgadget.sh`**

Change the header comment's list to add `3. adbd enabled at boot. Stock leaves it commented out and relies on pspapp to turn ADB on later; under waved pspapp never runs (docs/GUIDE_DEVICE_ACCESS.md, "Making ADB Survive a pspapp Failure").` Then:

```bash
INIT="${INIT:-/etc/init.d/S89usbgadget}"
BACKUP_DIR="${BACKUP_DIR:-/data/waved}"
```

replacing `INIT=/etc/init.d/S89usbgadget`. Add next to the other `ALREADY_*` checks:

```bash
ALREADY_ADB=1
grep -q '^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$' "$INIT" && ALREADY_ADB=0
```

extend the early exit to `… && [ "$ALREADY_CVOL" = "1" ] && [ "$ALREADY_ADB" = "1" ]` with message `already patched (p_volume + c_volume + hid.0 + adb at boot)`, replace `mkdir -p /data/waved` / `cp "$INIT" "/data/waved/S89usbgadget.bak.$(date +%s)"` with the `$BACKUP_DIR` versions, and add before the HID block:

```bash
# Stock start_usbgadget() has `#adbd_enable` and `# adbd_enable` in its two
# boot branches. Uncomment both so ADB comes up at every boot without pspapp.
if [ "$ALREADY_ADB" = "0" ]; then
    sed -i -e 's/^\([[:space:]]*\)#[[:space:]]*adbd_enable[[:space:]]*$/\1adbd_enable/' "$INIT"
    echo "patched adbd at boot"
fi
```

- [ ] **Step 4: Write the on-device patch test** (`waved/scripts/tests/test-patch-usbgadget-on-device.sh`)

```bash
#!/usr/bin/env bash
# Runs patch-stock-usbgadget.sh on a Wave against a /tmp copy of a stock
# S89usbgadget. Touches nothing outside /tmp on the Wave.
# Usage: test-patch-usbgadget-on-device.sh <stock S89usbgadget>
#   e.g. firmwares/fw_v1.1.6.6/extracted/rootfs/etc/init.d/S89usbgadget
set -euo pipefail
STOCK="${1:?usage: $0 <stock S89usbgadget>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
T="$(mktemp -d)"; trap 'rm -rf "$T"; adb shell "rm -rf /tmp/S89.test /tmp/S89.bak /tmp/patch-test.sh"' EXIT

adb push "$STOCK" /tmp/S89.test >/dev/null
adb push "$HERE/../patch-stock-usbgadget.sh" /tmp/patch-test.sh >/dev/null
adb shell "mkdir -p /tmp/S89.bak && INIT=/tmp/S89.test BACKUP_DIR=/tmp/S89.bak sh /tmp/patch-test.sh"
adb pull /tmp/S89.test "$T/once" >/dev/null
adb shell "INIT=/tmp/S89.test BACKUP_DIR=/tmp/S89.bak sh /tmp/patch-test.sh" | tr -d '\r' > "$T/second-run"
adb pull /tmp/S89.test "$T/twice" >/dev/null

fail=0
expect() { if eval "$2"; then echo "ok   $1"; else echo "FAIL $1"; fail=1; fi; }
expect no_commented_adbd_enable '! grep -q "^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*$" "$T/once"'
expect adbd_enabled_in_both_boot_branches '[ "$(grep -c "^[[:space:]]*adbd_enable[[:space:]]*$" "$T/once")" -eq "$(( $(grep -c "^[[:space:]]*adbd_enable[[:space:]]*$" "$STOCK") + 2 ))" ]'
expect p_volume_range 'grep -q "echo -16384 > functions/uac2.0/p_volume_min" "$T/once"'
expect hid_inserted_once '[ "$(grep -c "mkdir -p functions/hid.0" "$T/once")" -eq 1 ]'
expect second_run_is_a_no_op 'cmp -s "$T/once" "$T/twice" && grep -q "already patched" "$T/second-run"'
exit $fail
```

`chmod +x waved/scripts/tests/test-patch-usbgadget-on-device.sh`

- [ ] **Step 5: Run the on-device patch test** (the Wave is connected over adb; check `adb devices` first, and that the NPU session is not mid-run on the device)

Run: `waved/scripts/tests/test-patch-usbgadget-on-device.sh /Users/xaviergeerinck/Projects/insta360/firmwares/fw_v1.1.6.6/extracted/rootfs/etc/init.d/S89usbgadget`
Expected: five `ok` lines, exit 0. (The stock file is in the main checkout's ignored `firmwares/` tree; it is a vendor file and is not committed.)

- [ ] **Step 6: Rewrite `waved/deploy/install.sh`**

```bash
#!/bin/bash
# Install waved on an Insta360 Wave over adb. Safe to re-run: it upgrades an
# existing install and keeps its config.toml.
#
# Everything it changes is on /data; the read-only system partition is never
# touched, and deploy/revert.sh removes all of it:
#   /data/waved/                 waved, config.toml, splash.fb, iptables.rules
#   /data/overlay/upper/etc/     S60waved, S03splash, rsyslog.conf, and the
#                                patched S89usbgadget (the /etc overlay)
#   /data/overlay/upper/usr/lib  an empty libapplogrs.so (stock telemetry)
#
# The procedure follows docs/DISPLAY_HARDWARE.md#deploy: /data is small, so
# the new binary goes to /tmp first and replaces the old one only once it has
# arrived intact and waved has exited.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WAVED_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BINARY="${WAVED_BINARY:-${WAVED_DIR}/target/aarch64-unknown-linux-gnu/release/waved}"
UPPER=/data/overlay/upper
MARGIN_KB=20480   # room for logs and the recordings database after the swap

die() { echo "ERROR: $*" >&2; exit 1; }
on_wave() { adb shell "$1" | tr -d '\r'; }

strip_tool() {
    if [ -n "${OBJCOPY:-}" ]; then echo "$OBJCOPY"; return; fi
    local host tool
    host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
    tool="$(rustc --print sysroot 2>/dev/null)/lib/rustlib/${host}/bin/llvm-objcopy"
    [ -x "$tool" ] || die "llvm-objcopy not found. Run: rustup component add llvm-tools"
    echo "$tool"
}

local_md5() {
    if command -v md5sum >/dev/null 2>&1; then md5sum "$1" | cut -d' ' -f1; else md5 -q "$1"; fi
}

# --- Checks. Nothing on the Wave changes until all of them pass. ---
[ -f "$BINARY" ] || die "no waved binary at $BINARY
Build it first, from the waved/ directory:
  cross build --release --target aarch64-unknown-linux-gnu --features \"ui,capture-alsa\" -p waved"
[ "$(adb get-state 2>/dev/null || true)" = "device" ] || die "no Wave found on adb. Connect it over USB and enable ADB (Install guide, step 1)."

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
"$(strip_tool)" --strip-all "$BINARY" "$STAGE/waved"
SIZE_KB=$(( ($(wc -c < "$STAGE/waved") + 1023) / 1024 ))

FREE_KB="$(on_wave "df -k /data | tail -1" | awk '{print $4}')"
OLD_KB="$(on_wave "if [ -f /data/waved/waved ]; then du -k /data/waved/waved | cut -f1; else echo 0; fi")"
NEED_KB=$(( SIZE_KB + MARGIN_KB ))
[ $(( FREE_KB + OLD_KB )) -ge "$NEED_KB" ] || die "/data has $(( (FREE_KB + OLD_KB) / 1024 )) MB available and waved needs $(( NEED_KB / 1024 )) MB. Nothing was changed. See Troubleshooting: \"/data is full\"."

# --- Copy the new binary to /tmp and check it arrived intact. ---
echo "=== Copying waved ($(( SIZE_KB / 1024 )) MB) to the Wave ==="
adb push "$STAGE/waved" /tmp/waved.new >/dev/null
[ "$(on_wave "md5sum /tmp/waved.new" | cut -d' ' -f1)" = "$(local_md5 "$STAGE/waved")" ] \
    || die "the copy on the Wave does not match (md5). Nothing was changed; run install.sh again."

# --- Stop the stock app, the watchdog and waved, then swap. ---
echo "=== Stopping the stock app and any running waved ==="
on_wave "killall pspapp pspapp_daemon 2>/dev/null; \
kill \$(ps | awk '/S60waved start/ && !/awk/ {print \$1}') 2>/dev/null; \
killall waved 2>/dev/null; \
n=0; while pidof waved >/dev/null && [ \$n -lt 15 ]; do sleep 1; n=\$((n+1)); done; true" >/dev/null

echo "=== Installing waved ==="
[ "$(on_wave "mkdir -p /data/waved && rm -f /data/waved/waved && cp /tmp/waved.new /data/waved/waved && chmod 755 /data/waved/waved && rm -f /tmp/waved.new && echo ok")" = "ok" ] \
    || die "could not copy waved into /data/waved. The new binary is still in /tmp/waved.new; check 'adb shell df -h /data' and run install.sh again."

if [ "$(on_wave "[ -f /data/waved/config.toml ] && echo yes || echo no")" = "no" ]; then
    echo "=== Writing the default config.toml ==="
    adb push "${WAVED_DIR}/config/default.toml" /data/waved/config.toml >/dev/null
fi

echo "=== Installing start-up scripts, splash, logging and firewall rules ==="
on_wave "mkdir -p ${UPPER}/etc/init.d ${UPPER}/usr/lib /mnt/udisk/recordings" >/dev/null
adb push "${SCRIPT_DIR}/S60waved" "${UPPER}/etc/init.d/S60waved" >/dev/null
adb push "${SCRIPT_DIR}/S03splash" "${UPPER}/etc/init.d/S03splash" >/dev/null
adb push "${SCRIPT_DIR}/splash.fb" /data/waved/splash.fb >/dev/null
adb push "${SCRIPT_DIR}/rsyslog.conf" "${UPPER}/etc/rsyslog.conf" >/dev/null
adb push "${SCRIPT_DIR}/iptables.rules" /data/waved/iptables.rules >/dev/null
on_wave "chmod 0600 /data/waved/config.toml; \
chmod 755 ${UPPER}/etc/init.d/S60waved ${UPPER}/etc/init.d/S03splash; \
printf '#!/bin/sh\n' > ${UPPER}/etc/init.d/S60pspapp_daemon; chmod 755 ${UPPER}/etc/init.d/S60pspapp_daemon; \
: > ${UPPER}/usr/lib/libapplogrs.so" >/dev/null

echo "=== Patching the USB gadget: ADB at boot, volume ranges, HID controls ==="
adb push "${WAVED_DIR}/scripts/patch-stock-usbgadget.sh" /tmp/patch-usbgadget.sh >/dev/null
on_wave "sh /tmp/patch-usbgadget.sh; rm -f /tmp/patch-usbgadget.sh"

# Without this the Wave has no ADB after the reboot, and revert.sh can't reach it.
[ "$(on_wave "grep -c '^[[:space:]]*#[[:space:]]*adbd_enable[[:space:]]*\$' /etc/init.d/S89usbgadget")" = "0" ] \
    || die "ADB is not enabled at boot (/etc/init.d/S89usbgadget). Do not reboot; see Troubleshooting: \"ADB at boot\"."

echo "=== Done. Reboot the Wave to start waved: adb reboot ==="
```

- [ ] **Step 7: Rewrite `waved/deploy/revert.sh`**

```bash
#!/bin/bash
# Put a Wave back on its stock software: remove everything deploy/install.sh
# and over-the-air updates put on /data. The system partition was never
# changed, so after a reboot the stock app runs again. Deeper paths back
# (stock updater, raw restore) are in docs/UPDATES.md, "Paths back".
set -euo pipefail
UPPER=/data/overlay/upper

[ "$(adb get-state 2>/dev/null || true)" = "device" ] || { echo "ERROR: no Wave found on adb." >&2; exit 1; }

echo "=== Removing waved ==="
adb shell "rm -f \
${UPPER}/etc/init.d/S60waved ${UPPER}/etc/init.d/S60pspapp_daemon \
${UPPER}/etc/init.d/S03splash ${UPPER}/etc/init.d/S89usbgadget \
${UPPER}/etc/rsyslog.conf ${UPPER}/usr/lib/libapplogrs.so \
${UPPER}/etc/init.d/.S60waved.prev ${UPPER}/etc/init.d/.S60waved.new \
${UPPER}/etc/init.d/.S03splash.prev ${UPPER}/etc/init.d/.S03splash.new \
${UPPER}/etc/.rsyslog.conf.prev ${UPPER}/etc/.rsyslog.conf.new; \
rm -rf /data/waved"
echo "=== Done. Reboot the Wave to start the stock app: adb reboot ==="
echo "After the reboot ADB is off again, as on stock. To turn it on: sudo python3 waved/scripts/enable_adb.py"
```

- [ ] **Step 8: Run the host test and shellcheck**

Run: `waved/deploy/tests/test-deploy-scripts.sh && shellcheck waved/deploy/install.sh waved/deploy/revert.sh waved/deploy/tests/test-deploy-scripts.sh waved/scripts/tests/test-patch-usbgadget-on-device.sh`
Expected: every line `ok`, `all passed`; shellcheck prints nothing (fix any finding; `SC2016` on the deliberately single-quoted `check` expressions in the test may be disabled with a `# shellcheck disable=SC2016` line at the top of that file).

- [ ] **Step 9: Re-run the on-device patch test** (Step 5 command). Expected: five `ok`.

- [ ] **Step 10: Commit**

```bash
git add waved/deploy/install.sh waved/deploy/revert.sh waved/deploy/tests/test-deploy-scripts.sh waved/scripts/patch-stock-usbgadget.sh waved/scripts/tests/test-patch-usbgadget-on-device.sh
git commit -m "fix(deploy): install keeps ADB at boot and follows the proven deploy

install.sh pushed the unstripped binary straight over /data/waved/waved,
checked for config.toml on the host, and skipped rsyslog.conf, the splash
and the USB gadget patch. Worst, stock S89usbgadget leaves adbd off at boot
and only pspapp turns it on: after a reboot into waved a fresh Wave had no
ADB, so revert.sh could not reach it. The gadget patch now enables adbd at
boot and install.sh refuses to finish without it. revert.sh also removes
S03splash, rsyslog.conf and S89usbgadget.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

- [ ] **Step 11: Real install on the Wave: owner's go required.** Ask the owner; the NPU session may be using the device. With the go: build (`cd waved && WAVED_BUILD_ID=$(git rev-parse --short HEAD) cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved`; with sccache configured see `waved/docs/DISPLAY_HARDWARE.md#deploy`), run `./deploy/install.sh`, `adb reboot`, wait 60 s, then `adb devices` shows the Wave and `adb shell pidof waved` prints a PID. Record the result in the commit message of Task 6.

---

### Task 6: Getting started pages

**Files:**
- Create (overwrite stubs): `site/src/content/docs/start/{overview,requirements,install,first-setup,revert,troubleshooting}.md`

**Interfaces:**
- Consumes: slugs from Task 4 (`/insta360-custom-firmware/guides/updates/`, `…/research/security/`, `…/research/device-access/`, `…/reference/display-hardware/`), script behaviour from Task 5.

Rules for every page in Tasks 6–7: front matter `title` + one-sentence `description`; short sections; commands in fenced blocks exactly as they run; every claim checked against the source named in the outline; internal links with the base prefix.

- [ ] **Step 1: Write `start/install.md`** (full text; adjust only if Task 5 Step 11 showed a difference)

````md
---
title: Install
description: Build waved, put it on your Wave over USB, and reboot into it.
---

Installing takes about half an hour, most of it the first build. Everything
lands on the Wave's `/data` partition; the system partition is never touched,
and [one script puts stock back](/insta360-custom-firmware/start/revert/).

:::caution
This is unofficial firmware. It is tested on one Wave at a time by its
developers. Read [Revert to stock](/insta360-custom-firmware/start/revert/)
before you start.
:::

## 1. Enable ADB

ADB is how your computer talks to the Wave. Stock firmware keeps it off. With
the Wave on stock firmware and connected over USB-C:

```bash
pip3 install pyusb
sudo python3 waved/scripts/enable_adb.py
sleep 3
adb devices        # shows PSP_…    device
```

This ADB session lasts until the Wave reboots, which is enough: the installer
turns ADB on at every boot from then on.

:::danger
Never send the HID "factory devmode" command (`byte[5] = 0xEE`). It chains
into a factory reset that wipes recordings, cloud binding and region. Details:
[Device access](/insta360-custom-firmware/research/device-access/).
:::

## 2. Build

From the repository root:

```bash
cd waved
WAVED_BUILD_ID=$(git rev-parse --short HEAD) \
  cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
```

The first build downloads the cross-compilation image and takes a while.
To let the Wave install signed updates later, set `WAVED_UPDATE_PUBKEY`
before building ([Updates](/insta360-custom-firmware/guides/updates/)).

## 3. Install

```bash
./deploy/install.sh
```

The installer checks everything before it changes anything: that a Wave is
connected, that the build exists, and that `/data` has room. Then it

1. strips the binary, copies it to the Wave's `/tmp` and checks it arrived intact,
2. stops the stock app and swaps waved in,
3. writes a default `config.toml` if the Wave has none (an existing one is kept),
4. installs the start-up scripts, the boot splash, a logging fix that keeps `/data` from filling up, and the firewall rules,
5. patches the USB gadget so ADB, the volume controls and the call buttons work, and
6. refuses to finish unless ADB will be on after the reboot.

Running it again later upgrades waved and keeps your settings.

## 4. Reboot

```bash
adb reboot
```

The Wave shows the waved splash and then its home screen. Next:
[First setup](/insta360-custom-firmware/start/first-setup/).
````

- [ ] **Step 2: Write `start/requirements.md`**

Sections and facts:
- **Your Wave:** Insta360 Wave (model CINSABCA) on stock firmware 1.1.6.6 or 1.2.2.6 (sources: `docs/REPORT_DEVICE_DUMP.md` header, `firmwares/README.md`; check which versions waved has run on: `waved/docs/UPDATES.md` "Paths back" and `git log --grep=1.2.2.6`). A USB-C data cable.
- **Your computer:** macOS or Linux. Table of tools with the install command for each: `rustup` (Rust stable; `rustup component add llvm-tools`), Docker (running), `cross` (`cargo install cross --git https://github.com/cross-rs/cross`, why: `README.md` notes 0.2.5's rustup bug on macOS arm64), `adb` (`brew install android-platform-tools` / `apt install adb`), Python 3 with `pyusb`.
- **Disk:** the cross image plus a release build; say "several GB" unless measured.
- **Time:** about half an hour, mostly the first build.
- Windows: not supported for building (no source documents it); say so.

- [ ] **Step 3: Write `start/overview.md`**

Sections:
- **What it is:** `waved` is one Rust program that replaces the Wave's stock app (`pspapp`) and its telemetry; built from this repository. Not affiliated with Insta360.
- **What changes on your Wave, and what doesn't:** the list of paths from the top comment of `waved/deploy/install.sh`; the SquashFS system slots are never written (`waved/docs/UPDATES.md`, "Paths back", item 1).
- **What works today:** a table, feature → status → page. Derive each status from the sources, never from memory: home/recording/Files/settings screens (merges `746cd5f`, `1875241`, `aa40281`, `c5c0589`); macOS speakerphone and echo cancellation (`waved/docs/AUDIO_HARDWARE.md`); recordings and webhooks (`waved/docs/WEBHOOKS.md`, `waved/TODO.md` P0 items marked done); Wi-Fi provisioning (`waved/docs/WIFI.md`); Bluetooth audio and calls (merges `6c8e2dd`, `a856f70`; check `waved/docs/BLUETOOTH_PROTOCOL.md` for what is device-verified); Wave Controller (`b3df769`, `waved/docs/WAVE_CONTROLLER_PROTOCOL.md` status); signed updates: **host-tested only** (`waved/docs/UPDATES.md` "Status"); PIN lock (`9edc7c5`); battery, mic height (`242a133`); USB storage (check `git log --oneline feat/m3 | grep -i "usb storage\|workstream G"`; if absent, "not yet").
- **Why:** three bullets with links to `/insta360-custom-firmware/research/security/`.

- [ ] **Step 4: Write `start/first-setup.md`**

Sources: `waved/docs/WIFI.md` (provisioning flow: setup network, QR code on the screen, the setup page), `waved/config/default.toml` (every user-facing key: list them in a table with the default and one-line meaning, taken from the comments in that file and `waved/crates/waved-config/src`), `waved/docs/WEBHOOKS.md` (where recordings go), `waved/docs/UPDATES.md` "Owner's quick start" (the `security.ota_server` key), PIN rules (`TODO.md` decision D10). Sections: Join Wi-Fi · The setup page · `config.toml` (path `/data/waved/config.toml`, edit with `adb shell vi` or pull/push, takes effect after `adb shell killall waved` since the watchdog restarts it within ~3 s: `waved/CLAUDE.md`) · Send recordings to your server · Updates · PIN lock. Include the `wifi_qr` screenshot: `![The Wi-Fi setup QR code](../../../assets/screens/wifi_qr.png)`.

- [ ] **Step 5: Write `start/revert.md`**

Content: `./deploy/revert.sh` then `adb reboot`; the list of what it removes (from the script); "ADB is off again after the reboot, as on stock"; your recordings in `/mnt/udisk/recordings` are not deleted (check `revert.sh`: it does not touch `/mnt/udisk`, so say exactly that); going further back (stock updater, raw restore): link `/insta360-custom-firmware/guides/updates/#paths-back-least-invasive-first`; installing again later: run `install.sh`.

- [ ] **Step 6: Write `start/troubleshooting.md`**

One section per problem, each with the check command and the fix, sourced:
- `adb devices` is empty: `docs/GUIDE_DEVICE_ACCESS.md` "ADB Device Not Showing".
- "/data is full": `adb shell df -h /data`; stock `psp.log` cause and the `rsyslog.conf` fix: `waved/docs/DISPLAY_HARDWARE.md` deploy notes.
- "ADB at boot" (install.sh refused): do not reboot. Re-run `./deploy/install.sh`; if it still refuses, show `adb shell grep -n adbd_enable /etc/init.d/S89usbgadget` (the two boot-branch lines must not start with `#`) and link the guide section "Making ADB Survive a pspapp Failure" (`/insta360-custom-firmware/research/device-access/#making-adb-survive-a-pspapp-failure`).
- waved does not start: `adb shell tail -50 /data/waved/waved.log`; the watchdog restarts it every ~3 s (`waved/deploy/S60waved`).
- Build fails with sccache: the `RUSTC_WRAPPER=` passthrough note in `waved/docs/DISPLAY_HARDWARE.md`.
- The checksum didn't match: run `install.sh` again (nothing was changed).

- [ ] **Step 7: Build and check links**

Run: `cd site && npm run build 2>&1 | tail -15`
Expected: `[build] Complete!`.

- [ ] **Step 8: Commit**

```bash
git add site/src/content/docs/start
git commit -m "docs(site): getting started: overview, requirements, install, setup, revert, troubleshooting

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Features tour, How it works, Development

**Files:**
- Create (overwrite stubs): `site/src/content/docs/features.md`, `site/src/content/docs/how/{architecture,hardware}.md`, `site/src/content/docs/dev/{building,testing,structure,contributing}.md`

**Interfaces:**
- Consumes: screenshots in `src/assets/screens/` (Task 4 `screens` list); slugs from Task 4.

- [ ] **Step 1: Write `features.md`**

One section per feature: two to four sentences, one screenshot, the status (same sources as `start/overview.md`), and a link to the deeper page. Images use `../../assets/screens/<name>.png` (the page sits at `src/content/docs/features.md`). Sections and screenshots: The dial and home screen (`home`, `dial_live_talker`; ring states from `waved/docs/ui/` and `waved-ui` source) · Recording (`recording`, `recording_hold_stop`; hold Stop for 1 s, pre-recording "includes last 30 s": `TODO.md` D1, D3) · Files (`files`, `file_detail`) · Audio modes (`screen_audio_mode`; the six modes: `TODO.md` D2, check against `waved-state` `AudioMode`) · Bluetooth audio and calls (`screen_bluetooth`, `screen_call_active`) · Uploads and webhooks (`screen_uploads`) · Updates (`update_installing`, host-tested only) · PIN lock (`screen_pin`; what it locks: D10) · Mic height (`screen_mic_height`) · Wave Controller (no screenshot; `waved/docs/WAVE_CONTROLLER_PROTOCOL.md`).

- [ ] **Step 2: Write `how/architecture.md`**

Sections (facts from the named sources, read them first):
- **One program:** `waved` (tokio), started and restarted by `/etc/init.d/S60waved` (`waved/deploy/S60waved`), splash by `S03splash`; stock init and rootfs untouched.
- **The state bus:** `waved-state` (read the crate's `lib.rs` module docs) — services publish state and receive commands.
- **Services:** a table, crate → job, for the production crates only (`waved`, `waved-state`, `waved-config`, `waved-settings`, `waved-audio`, `waved-meeting`, `waved-ui`, `waved-bt`, `waved-uart`, `waved-mcu`, `waved-mavlink`, `waved-hid`, `waved-wifi`, `waved-provision`, `waved-system`, `waved-update`). Read each crate's `src/lib.rs` header (and README where one exists) for its job. Then one line naming the tools and probes (`waved-*-probe`, `waved-audio-bench`, `waved-doa-tap`, `waved-mic-map`, `waved-capture-record`, `waved-device-test`, `waved-npu*`).
- **The audio path:** mics → capture → echo cancellation, beamforming, direction of arrival → USB (UAC2), recording, Bluetooth. Take the stages and names from `waved/crates/waved-audio/README.md` and `waved/docs/AUDIO_HARDWARE.md`; do not reuse the README's "TEA SDK via FFI" line unless the code still does that (`git grep -n -i tea -- waved/crates/waved-audio/src | head`).
- **The screen:** LVGL on the round `fb_ili79400` panel (`waved/docs/DISPLAY_HARDWARE.md`), snapshot tests render every screen to PNG (`waved/crates/waved-ui/golden/`).
- **Recordings:** WAV + SQLite + durable webhook queue (`waved/docs/WEBHOOKS.md`).
- **Updates:** signed `.wup` packages from your own server, trial boots and fall-back (`waved/docs/UPDATES.md`).
- One Mermaid diagram: hardware ports (ttyS1 Bluetooth module, ttyS4 MCU, USB gadget, ALSA, fb0) → services → state bus → UI.

- [ ] **Step 3: Write `how/hardware.md`**

A table from the reference docs only (Global Constraints): SoC and CPU/NPU/DSP (`COMPUTE_HARDWARE.md`), RAM and eMMC layout (`docs/REPORT_DEVICE_DUMP.md`), mic array and speakers (`AUDIO_HARDWARE.md`), display and touch (`DISPLAY_HARDWARE.md`), Bluetooth module and its UART (`BLUETOOTH_PROTOCOL.md`), MCU, battery, lift motor (`MCU_PROTOCOL.md`), Wi-Fi chip (`WIFI.md`; the driver logs as `atbm`), USB gadget functions (`AUDIO_HARDWARE.md` UAC2 section). Each row links to its reference page.

- [ ] **Step 4: Write the four `dev/` pages**

- `dev/building.md`: the build command (Task 6 install page), `Cross.toml` and `Dockerfile.cross-aarch64` (what the image provides: read their comments), `WAVED_BUILD_ID`, `WAVED_UPDATE_PUBKEY`, the sccache note, the manual deploy procedure link (`/insta360-custom-firmware/reference/display-hardware/#deploy`).
- `dev/testing.md`: host unit tests (`cargo test -p <crate> --lib`); crates that need Linux (`waved-meeting`, `waved`: `scripts/linux-test.sh`, `waved/CLAUDE.md`); UI snapshot tests and goldens; on-device scripts (`waved/scripts/smoke.sh`, `hil.sh`, `test-*.sh`), `waved-device-test`. Verify each command exists before listing it (`ls waved/scripts`, `git grep -n 'simulate' -- waved/crates/waved/Cargo.toml`); drop any README layer that no longer exists.
- `dev/structure.md`: the top-level tree (`waved/`, `docs/`, `site/`, `scripts/`, `firmwares/`), what lives in `waved/docs` vs `docs/`, and that the site syncs both.
- `dev/contributing.md`: branches and PRs; conventional commit messages (as in `git log`); docs: edit the Markdown in `docs/` or `waved/docs/`, the site picks it up, new docs need a line in `site/sync/manifest.mjs`; tests to run; **never put real SSIDs, MACs, IPs, serials or device IDs in fixtures or docs** (use `Office-5G`, `aa:bb:cc:dd:ee:ff`, `192.168.1.x`, `IW1A2B3C4D`); security reports: open a private advisory on GitHub.

- [ ] **Step 5: Build** (`cd site && npm run build`; expected `[build] Complete!`).

- [ ] **Step 6: Commit**

```bash
git add site/src/content/docs/features.md site/src/content/docs/how site/src/content/docs/dev
git commit -m "docs(site): features tour, how it works, development

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Landing page

**Files:**
- Modify: `site/src/content/docs/index.mdx` (replace)

- [ ] **Step 1: Write `index.mdx`**

```mdx
---
title: Open firmware for the Insta360 Wave
description: waved replaces the Insta360 Wave's stock software with one open program. No telemetry, recordings that stay yours, and one script back to stock.
template: splash
hero:
  tagline: One open program in place of the stock software. No telemetry, your recordings stay yours, and one script puts stock back.
  image:
    file: ../../assets/screens/home.png
    alt: The waved home screen on the Wave's round display
  actions:
    - text: Install
      link: /insta360-custom-firmware/start/install/
      icon: right-arrow
    - text: Read the docs
      link: /insta360-custom-firmware/start/overview/
      variant: secondary
    - text: GitHub
      link: https://github.com/XavierGeerinck/insta360-custom-firmware
      icon: github
      variant: minimal
---

import { Card, CardGrid, LinkCard } from '@astrojs/starlight/components';
import { Image } from 'astro:assets';
import recording from '../../assets/screens/recording.png';
import audioMode from '../../assets/screens/screen_audio_mode.png';
import files from '../../assets/screens/files.png';
import call from '../../assets/screens/screen_call_active.png';
import menu from '../../assets/screens/screen_menu.png';

## Why

The stock firmware sends more than 35 kinds of usage events to ByteDance's
DataFinder service, tagged with the Wave's serial number. It routes meeting
audio, transcripts and voiceprints to Insta360's cloud, signs its updates with a
512-bit RSA key, and older releases opened a root shell over telnet with the
password `123456`. The evidence is in the
[Security Q&A](/insta360-custom-firmware/research/security/).

## What you get

<CardGrid>
	<Card title="No telemetry" icon="seti:lock">
		The stock app and its ByteDance telemetry library never run. waved talks
		only to the time servers in its config and to the webhook and update
		servers you set up.
	</Card>
	<Card title="Your recordings" icon="document">
		Meetings are recorded on the Wave and delivered only to a webhook you
		choose, with retries that survive a restart.
	</Card>
	<Card title="A real speakerphone" icon="laptop">
		Plug it into a Mac or PC and it is a USB speakerphone with echo
		cancellation and beamforming. No Insta360 app needed.
	</Card>
	<Card title="Bluetooth audio and calls" icon="phone">
		Pair a phone for music and calls, and answer or decline on the Wave.
	</Card>
	<Card title="Signed updates you host" icon="rocket">
		Update packages are signed with your own key and served from your own
		server, with a trial boot and an automatic way back.
	</Card>
	<Card title="One script back to stock" icon="refresh">
		The installer only writes to `/data`. `revert.sh` removes it all and the
		stock app starts again after a reboot.
	</Card>
</CardGrid>

## On the screen

<div class="screens">
	<Image src={recording} alt="Recording, with the talker's direction on the dial" />
	<Image src={audioMode} alt="Choosing an audio mode" />
	<Image src={files} alt="Recordings kept on the Wave" />
	<Image src={call} alt="A phone call over Bluetooth" />
	<Image src={menu} alt="The menu" />
</div>

These are rendered by the firmware's own UI code in its snapshot tests.

## How it works

waved is a single Rust program. Each piece of hardware (the microphones and
speakers, the screen, the Bluetooth module, the battery and lift controller,
USB and Wi-Fi) has its own service, and the services share state through one
in-process bus. The init script starts it at boot and restarts it if it exits.

<LinkCard title="How it works" href="/insta360-custom-firmware/how/architecture/" />

## Status

Unofficial and not affiliated with Insta360. Most features are verified on a
Wave; the ones that are not yet say so.

<LinkCard title="What works today" href="/insta360-custom-firmware/start/overview/" />

<style>{`
.screens { display: grid; grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr)); gap: 1rem; }
.screens img { border-radius: 50%; width: 100%; height: auto; }
`}</style>
```

Then check every card and paragraph against its source and correct the wording wherever the source says otherwise:
- No telemetry: `waved/deploy/install.sh` (empty `libapplogrs.so`, stock app stubbed); `waved/config/default.toml` (what is contacted by default; if anything besides the NTP servers is, name it).
- Your recordings: `waved/docs/WEBHOOKS.md` (durable queue, retries).
- A real speakerphone: `waved/docs/AUDIO_HARDWARE.md` (UAC2, AEC, beamforming).
- Bluetooth: `waved/docs/BLUETOOTH_PROTOCOL.md`, `waved-state` call commands (`AnswerCall`, `DeclineCall`); if decline is not wired to the screen, drop "or decline".
- Updates: `waved/docs/UPDATES.md`; it is host-tested only, so append "Tested on the host so far." while its Status line says so.
- Revert: `waved/deploy/revert.sh`, `waved/docs/UPDATES.md` "Paths back" item 1.
- How it works: consistent with `how/architecture.md` (Task 7).
- Status: consistent with the overview's status table (Task 6).

Check that every `icon` name exists in Starlight's icon set (build fails or warns on unknown icons); swap any that don't.

- [ ] **Step 2: Build**

Run: `cd site && npm run build 2>&1 | tail -5`
Expected: `[build] Complete!`.

- [ ] **Step 3: Commit**

```bash
git add site/src/content/docs/index.mdx
git commit -m "docs(site): landing page

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: README and LICENSE

**Files:**
- Modify: `README.md` (rewrite)
- Create: `LICENSE`

- [ ] **Step 1: Write `LICENSE`**: the standard MIT text, `Copyright (c) 2026 Xavier Geerinck`.

- [ ] **Step 2: Rewrite `README.md`**

```md
# Wave Open Firmware

Open firmware for the **Insta360 Wave** speakerphone. `waved`, one Rust
program, replaces the stock software: no telemetry, recordings that stay
yours, and one script back to stock.

**Documentation: https://xaviergeerinck.github.io/insta360-custom-firmware/**

- [Install](https://xaviergeerinck.github.io/insta360-custom-firmware/start/install/)
- [What works today](https://xaviergeerinck.github.io/insta360-custom-firmware/start/overview/)
- [How it works](https://xaviergeerinck.github.io/insta360-custom-firmware/how/architecture/)
- [Reverse-engineering research](https://xaviergeerinck.github.io/insta360-custom-firmware/research/report/)

## Why

The stock firmware sends more than 35 kinds of usage events to ByteDance's
DataFinder service, routes meeting audio and transcripts to Insta360's cloud,
and signs its updates with a 512-bit RSA key. See the
[Security Q&A](docs/QUESTIONS_SECURITY.md).

## Quick start

```bash
sudo python3 waved/scripts/enable_adb.py       # ADB, until the next reboot
cd waved
cross build --release --target aarch64-unknown-linux-gnu --features "ui,capture-alsa" -p waved
./deploy/install.sh && adb reboot
```

Requirements and every step explained: [Install](https://xaviergeerinck.github.io/insta360-custom-firmware/start/install/).
Back to stock: `./deploy/revert.sh && adb reboot`.

## Repository

| Path | What |
|---|---|
| `waved/` | the firmware (Rust workspace), deploy scripts, hardware docs |
| `docs/` | reverse-engineering reports and guides |
| `site/` | the documentation site |
| `scripts/` | firmware download, decompilation and comparison tools |

The decompiled stock binaries are not distributed. `scripts/decompile_version.sh`
regenerates them from a stock firmware image with Ghidra.

## Status

Unofficial and not affiliated with Insta360. Use at your own risk; the install
never touches the system partition and `revert.sh` restores stock.

## License

[MIT](LICENSE)
```

Check `scripts/decompile_version.sh`'s header for what it needs and fix the sentence if it differs.

- [ ] **Step 3: Commit**

```bash
git add README.md LICENSE
git commit -m "docs: short README pointing at the site, add the MIT license file

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Pages workflow

**Files:**
- Create: `.github/workflows/pages.yml`

- [ ] **Step 1: Write the workflow**

```yaml
name: Site

on:
  push:
    branches: [main]
    paths:
      - docs/**
      - waved/docs/**
      - waved/crates/waved-ui/golden/**
      - site/**
      - .github/workflows/pages.yml
  pull_request:
    paths:
      - docs/**
      - waved/docs/**
      - waved/crates/waved-ui/golden/**
      - site/**
      - .github/workflows/pages.yml
  workflow_dispatch:

permissions:
  contents: read

concurrency:
  group: pages-${{ github.ref }}
  # Superseded PR builds can go; a Pages deployment must never be cut off midway.
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}

jobs:
  build:
    runs-on: ubuntu-latest
    defaults:
      run:
        working-directory: site
    steps:
      - uses: actions/checkout@v5
      - uses: actions/setup-node@v5
        with:
          node-version: lts/*
          cache: npm
          cache-dependency-path: site/package-lock.json
      - run: npm ci
      - run: npm test
      - run: npm run build
      - if: github.event_name != 'pull_request'
        uses: actions/upload-pages-artifact@v4
        with:
          path: site/dist

  deploy:
    if: github.event_name != 'pull_request'
    needs: build
    runs-on: ubuntu-latest
    permissions:
      pages: write
      id-token: write
    environment:
      name: github-pages
      url: ${{ steps.deployment.outputs.page_url }}
    steps:
      - id: deployment
        uses: actions/deploy-pages@v4
```

- [ ] **Step 2: Check the action versions are current** (`gh api repos/actions/checkout/releases/latest --jq .tag_name`, same for `setup-node`, `upload-pages-artifact`, `deploy-pages`); use each one's latest major.

- [ ] **Step 3: Lint**

Run: `actionlint .github/workflows/pages.yml`
Expected: no output.

- [ ] **Step 4: Simulate the CI build locally from a clean install**

Run: `cd site && rm -rf node_modules dist && npm ci && npm test && npm run build 2>&1 | tail -3`
Expected: tests pass, `[build] Complete!`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/pages.yml
git commit -m "ci: build the site on PRs, deploy it to GitHub Pages from main

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Tree scrub

**Files:**
- Modify: every tracked file containing a value from `~/.config/waved/public-scrub.txt` (expected: `docs/REPORT_DEVICE_DUMP.md`, `backups/README.md`, `docs/superpowers/specs/2026-04-08-waved-flash-deployment-design.md`, `waved/crates/waved-wifi/src/wpa.rs`, `waved/crates/waved-bt/src/{event,lib,state}.rs`, `waved/docs/BLUETOOTH_PROTOCOL.md`, `waved/docs/superpowers/plans/2026-10-13-m3-i1-i3-i4-bt-control.md`), `.gitignore`, `docs/REPORT.md`
- Delete: `decompiled/`

- [ ] **Step 1: Write the check that must end at zero**

Create `scripts/check-scrub.sh` (it is useful again in Task 13):

```bash
#!/usr/bin/env bash
# Counts, for each value in the scrub file, the tracked files that still
# contain it. Prints nothing and exits 0 when the tree is clean. The values
# themselves are never printed.
# Usage: scripts/check-scrub.sh [scrub file]
set -uo pipefail
FILE="${1:-$HOME/.config/waved/public-scrub.txt}"
dirty=0; n=0
while IFS= read -r line || [ -n "$line" ]; do
    [ -z "$line" ] && continue
    n=$((n+1))
    from="${line%%==>*}"
    if [[ "$from" == regex:* ]]; then hits=$(git grep -l -E -- "${from#regex:}" | wc -l)
    else hits=$(git grep -l -F -- "$from" | wc -l); fi
    if [ "$hits" -gt 0 ]; then echo "rule $n: $hits file(s)"; dirty=1; fi
done < "$FILE"
[ -e decompiled ] && git ls-files --error-unmatch decompiled >/dev/null 2>&1 && { echo "decompiled/ is tracked"; dirty=1; }
exit $dirty
```

`chmod +x scripts/check-scrub.sh`

- [ ] **Step 2: Run it to verify it fails**

Run: `scripts/check-scrub.sh`
Expected: several `rule N: K file(s)` lines and `decompiled/ is tracked`; exit 1.

- [ ] **Step 3: Apply the replacements**

```bash
while IFS= read -r line || [ -n "$line" ]; do
  [ -z "$line" ] && continue
  from="${line%%==>*}"; to="${line##*==>}"
  if [[ "$from" == regex:* ]]; then
    FROM="${from#regex:}" TO="$to" git grep -l -z -E -- "${from#regex:}" | xargs -0 -r perl -pi -e 's/$ENV{FROM}/$ENV{TO}/g'
  else
    FROM="$from" TO="$to" git grep -l -z -F -- "$from" | xargs -0 -r perl -pi -e 's/\Q$ENV{FROM}\E/$ENV{TO}/g'
  fi
done < ~/.config/waved/public-scrub.txt
```

(The regex rule's replacement `…0xE5]` contains no regex metacharacters that Perl would interpret in the replacement part.)

- [ ] **Step 4: Remove the decompiled sources and keep them out**

```bash
git rm -r -q --cached decompiled
rm -rf decompiled
printf '\n# Decompiled stock binaries: regenerate with scripts/decompile_version.sh,\n# never commit (Insta360 code).\n/decompiled/\n' >> .gitignore
```

Add under the title of `docs/REPORT.md`:

```md
> The decompiled C these reports cite (`decompiled/*.c`, with line numbers) is not
> distributed. `scripts/decompile_version.sh` regenerates it from a stock firmware
> image with Ghidra; line numbers match the 1.1.6.6 image.
```

Check the last claim (`git log --oneline -- decompiled | tail -1` and `scripts/decompile_version.sh`) and adjust the version if it differs.

- [ ] **Step 5: Run the check and the affected tests**

Run: `scripts/check-scrub.sh && cd waved && cargo test -p waved-bt --lib && cargo test -p waved-wifi --lib`
Expected: no output from the check (exit 0); both test runs `test result: ok`. If `waved-bt` fails on the frame fixture, the checksum byte did not survive: the placeholder MAC's bytes must sum to `0x26` mod 256.

- [ ] **Step 6: Run gitleaks on the working tree**

Run: `gitleaks dir . --no-banner --redact 2>&1 | tail -3`
Expected: only the two known false positives (the webhook test's idempotency key, the vendor placeholder key in `docs/analysis_reports/libins_entry_Binary_Analysis.md`), or `no leaks found`.

- [ ] **Step 7: Commit**

```bash
git add -A -- . ':!site'
git status --short | head -30   # review: only the scrubbed files, .gitignore, REPORT.md, the check script, the decompiled/ deletions
git commit -m "chore: remove personal identifiers and the decompiled vendor code

Replaces a home Wi-Fi name and BSSID, the device serial, a Wi-Fi MAC, a LAN
IP and a phone Bluetooth address with placeholders (the BT fixture keeps its
frame checksum). Stops tracking decompiled/: Insta360's code, regenerated by
scripts/decompile_version.sh. The reports keep every finding.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: Verify the site and review the branch

- [ ] **Step 1: Preview**

Run: `cd site && npm run build && npm run preview -- --port 4321` (background)

- [ ] **Step 2: Screenshot and check** (headless Chrome or the browser skill): `http://localhost:4321/insta360-custom-firmware/` (hero, cards, round screenshots), `…/start/install/`, `…/research/debug-pipeline/` (Mermaid diagrams render as SVG, not code), `…/research/npu/` (sidebar NPU group expands), search for "MAVLink" returns the MCU protocol page, the "Edit page" link of `…/reference/mcu-protocol/` points at `waved/docs/MCU_PROTOCOL.md`. Fix and commit anything wrong.

- [ ] **Step 3: Full checks**

Run: `cd site && npm test && npm run build 2>&1 | tail -2; cd .. && waved/deploy/tests/test-deploy-scripts.sh | tail -1 && scripts/check-scrub.sh && actionlint`
Expected: all green, `all passed`, no scrub output.

- [ ] **Step 4: NPU docs untouched**

Run: `git diff --stat feat/m3 -- docs/npu`
Expected: no output.

- [ ] **Step 5: Whole-branch review** with superpowers:requesting-code-review (fresh reviewer, most capable model) over `git diff feat/m3...feat/public-site`, focusing on the Review Focus list. Fix findings, then merge `feat/public-site` into `feat/m3` (owner's call on timing; it deletes `decompiled/` from the main checkout, so first `cp -R decompiled ~/insta360-decompiled-backup` there if the owner wants a local copy).

---

### Task 13: History rewrite (after `feat/m3` is on `main`)

Preconditions (check each, stop if one fails): `git log origin/main --oneline | grep -c "public site\|sync the repository docs"` ≥ 1; `git branch --list 'worktree-agent-*'` merged branches deleted; no session mid-commit. Everything here happens in a scratch directory, not the working repository.

- [ ] **Step 1: Mirror clone**

```bash
mkdir -p ~/wave-public && cd ~/wave-public
git clone --mirror https://github.com/XavierGeerinck/insta360-custom-firmware.git wave.git
```

- [ ] **Step 2: Rewrite**

```bash
cd ~/wave-public/wave.git
git filter-repo \
  --invert-paths --path decompiled/ --path firmware/ --path waved/rust_out \
  --replace-text ~/.config/waved/public-scrub.txt \
  --replace-message ~/.config/waved/public-scrub.txt
```

Expected: `New history written`, `Completely finished`.

- [ ] **Step 3: Verify, all must be zero**

```bash
cd ~/wave-public && rm -rf check && git clone -q wave.git check && cd check
git log --all --oneline -- decompiled firmware waved/rust_out | wc -l          # 0
gitleaks git --log-opts="--all" --no-banner --redact . 2>&1 | grep -c 'Finding:'  # ≤ 2, the known false positives only
while IFS= read -r line; do from="${line%%==>*}"; [[ "$from" == regex:* ]] && from="${from#regex:}" && from="${from//\\/}"; \
  printf '%s ' "$(git log --all -p --format=%B | grep -c -F -- "$from")"; done < ~/.config/waved/public-scrub.txt; echo   # all 0
git log --all -p | grep -c -i -E 'e4.?7f.?00.?ba.?ad'                            # 0
```

- [ ] **Step 4: The rewritten head still builds and tests**

```bash
cd ~/wave-public/check
(cd waved && cargo test -p waved-bt --lib && cargo test -p waved-wifi --lib)
(cd site && npm ci && npm test && npm run build)
scripts/check-scrub.sh
```

Expected: all pass.

---

### Task 14: Publish (each step needs the owner's go)

- [ ] **Step 1: Rename the current repo** (go #1)

```bash
gh repo rename insta360-custom-firmware-archive -R XavierGeerinck/insta360-custom-firmware --yes
```

- [ ] **Step 2: Re-point every local checkout at the archive, before the new repo exists**

```bash
git -C /Users/xaviergeerinck/Projects/insta360 remote set-url origin https://github.com/XavierGeerinck/insta360-custom-firmware-archive.git
git -C /Users/xaviergeerinck/Projects/insta360 remote get-url origin
```

Expected: the URL ends in `-archive.git`. Worktrees share this config. Ask the owner whether any other clone exists (another machine) and have it re-pointed too. Do not continue until confirmed.

- [ ] **Step 3: Create the public repo and push** (go #2)

```bash
gh repo create XavierGeerinck/insta360-custom-firmware --public \
  --description "Open firmware for the Insta360 Wave: no telemetry, recordings that stay yours, one script back to stock" \
  --homepage https://xaviergeerinck.github.io/insta360-custom-firmware/
cd ~/wave-public/wave.git
git push https://github.com/XavierGeerinck/insta360-custom-firmware.git 'refs/heads/*:refs/heads/*' 'refs/tags/*:refs/tags/*'
```

(Not `--mirror`: the mirror holds GitHub's hidden `refs/pull/*`, which a push rejects.)

- [ ] **Step 4: Enable Pages and deploy** (go #3)

```bash
gh api -X POST repos/XavierGeerinck/insta360-custom-firmware/pages -f build_type=workflow
gh workflow run pages.yml -R XavierGeerinck/insta360-custom-firmware
gh run watch -R XavierGeerinck/insta360-custom-firmware "$(gh run list -R XavierGeerinck/insta360-custom-firmware -w Site -L 1 --json databaseId --jq '.[0].databaseId')"
```

Expected: both jobs succeed.

- [ ] **Step 5: Check the live site**

```bash
for p in "" start/install/ research/security/ research/npu/ reference/mcu-protocol/; do
  printf '%s %s\n' "$(curl -s -o /dev/null -w '%{http_code}' "https://xaviergeerinck.github.io/insta360-custom-firmware/$p")" "$p"; done
```

Expected: `200` for every line.

- [ ] **Step 6: Fresh local clone**

```bash
git clone https://github.com/XavierGeerinck/insta360-custom-firmware.git ~/Projects/insta360-public
```

Tell the owner: the old checkout now tracks the private archive; new work goes in the fresh clone (copy untracked material such as `firmwares/`, `models/`, `ghidra_projects/` across by hand if wanted).
