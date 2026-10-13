import path from 'node:path';
import { createFenceTracker } from './fences.mjs';

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
	const isFenced = createFenceTracker();
	let title = null;
	for (let i = 0; i < lines.length; i++) {
		if (isFenced(lines[i])) continue;
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
