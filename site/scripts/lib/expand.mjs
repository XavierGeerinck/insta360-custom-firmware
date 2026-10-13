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
