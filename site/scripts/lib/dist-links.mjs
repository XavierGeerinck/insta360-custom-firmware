// Maps a link found in built HTML to the file in dist/ it must resolve to.
// Starlight's link validator covers its own pages only; this covers the
// hand-built ones (src/pages).

const SKIP = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i; // https:, mailto:, data:, //host

/**
 * @returns {{file: string, hash: string} | {error: string} | null}
 *   file is relative to dist/ ('' means the page itself); null means not ours to check.
 */
export function targetFor(url, base) {
	if (SKIP.test(url)) return null;
	const [withoutHash, hash = ''] = url.split('#');
	const path = withoutHash.split('?')[0];
	if (path === '') return { file: '', hash };
	if (path !== base && !path.startsWith(`${base}/`)) return { error: 'outside the site base' };
	const rest = path.slice(base.length).replace(/^\//, '');
	let file;
	if (rest === '' || rest.endsWith('/')) file = `${rest}index.html`;
	else if (/\.[a-z0-9]+$/i.test(rest)) file = rest;
	else file = `${rest}/index.html`;
	return { file, hash };
}

export function linksIn(html) {
	return [...html.matchAll(/\s(?:href|src)=["']([^"']+)["']/g)].map((m) => m[1]);
}

export function idsIn(html) {
	return new Set([...html.matchAll(/\sid=["']([^"']+)["']/g)].map((m) => m[1]));
}
