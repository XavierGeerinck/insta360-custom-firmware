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

// Expand first: a manifest entry that names a missing file throws here, before
// anything below deletes the last good output.
const pages = expandEntries(entries, REPO_ROOT);

for (const dir of ['guides', 'reference', 'research']) fs.rmSync(path.join(CONTENT, dir), { recursive: true, force: true });
fs.rmSync(ASSETS, { recursive: true, force: true });
fs.rmSync(SCREENS_OUT, { recursive: true, force: true });

const urlBySource = new Map(pages.map((p) => [p.source, `${BASE}/${p.slug}/`]));
const warnings = [];

for (const p of pages) {
	const { markdown, repoLinks, assets } = rewriteLinks(fs.readFileSync(path.join(REPO_ROOT, p.source), 'utf8'), {
		sourcePath: p.source,
		urlFor: (rp) => urlBySource.get(rp),
		isDir: (rp) => fs.statSync(path.join(REPO_ROOT, rp), { throwIfNoEntry: false })?.isDirectory() ?? false,
		assetUrlFor: (rp) => `${BASE}/repo-assets/${encodeURI(rp)}`,
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
