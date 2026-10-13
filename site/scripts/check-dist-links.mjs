// After `astro build`: every internal link and asset on the hand-built pages
// must exist in dist/, and every #anchor must exist on its target page.
// Fails the build otherwise (Starlight's validator does not see these pages).
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../site.config.mjs';
import { targetFor, linksIn, idsIn } from './lib/dist-links.mjs';

const DIST = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../dist');
const PAGES = ['index.html'];

const problems = [];
let checked = 0;
for (const page of PAGES) {
	const html = fs.readFileSync(path.join(DIST, page), 'utf8');
	for (const url of new Set(linksIn(html))) {
		const t = targetFor(url, BASE);
		if (t === null) continue;
		checked++;
		if ('error' in t) {
			problems.push(`${page}: ${url} (${t.error})`);
			continue;
		}
		const file = t.file === '' ? page : t.file;
		const full = path.join(DIST, file);
		if (!fs.existsSync(full)) {
			problems.push(`${page}: ${url} (no ${file} in dist)`);
			continue;
		}
		if (t.hash && !idsIn(fs.readFileSync(full, 'utf8')).has(t.hash)) {
			problems.push(`${page}: ${url} (no #${t.hash} on ${file})`);
		}
	}
}

if (problems.length) {
	console.error(`check-dist-links: ${problems.length} broken link(s):`);
	for (const p of problems) console.error(`  ${p}`);
	process.exit(1);
}
console.log(`check-dist-links: ${checked} links on ${PAGES.length} page(s) resolve`);
