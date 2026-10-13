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
