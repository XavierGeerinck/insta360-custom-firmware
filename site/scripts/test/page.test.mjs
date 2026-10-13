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
