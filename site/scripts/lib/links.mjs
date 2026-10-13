import path from 'node:path';
import { createFenceTracker } from './fences.mjs';

// A scheme (http:, mailto:, …) or a protocol-relative URL.
const EXTERNAL = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;
// [label](target "title") and ![alt](target). Labels may hold one level of
// nested brackets; targets may be wrapped in <>.
const LINK = /(!?)\[((?:[^[\]]|\[[^[\]]*\])*)\]\(\s*<?([^)\s>]+)>?((?:\s+"[^"]*")?)\s*\)/g;
const CODE_SPAN = /(`+)(?:(?!\1)[\s\S])*?\1/g;

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
export function rewriteLinks(markdown, ctx) {
	const repoLinks = [];
	const assets = [];
	const isFenced = createFenceTracker();
	const lines = markdown.split('\n').map((line) =>
		isFenced(line) ? line : rewriteLine(line, ctx, repoLinks, assets),
	);
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
