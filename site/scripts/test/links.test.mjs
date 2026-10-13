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
