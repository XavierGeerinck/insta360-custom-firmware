import { test } from 'node:test';
import assert from 'node:assert/strict';
import { targetFor, linksIn, idsIn } from '../lib/dist-links.mjs';

const BASE = '/insta360-custom-firmware';

test('page_url_maps_to_index_html', () => {
	assert.deepEqual(targetFor(`${BASE}/start/install/`, BASE), { file: 'start/install/index.html', hash: '' });
});

test('root_maps_to_index_html', () => {
	assert.deepEqual(targetFor(`${BASE}/`, BASE), { file: 'index.html', hash: '' });
	assert.deepEqual(targetFor(BASE, BASE), { file: 'index.html', hash: '' });
});

test('hash_is_split_off', () => {
	assert.deepEqual(targetFor(`${BASE}/features/#recording`, BASE), { file: 'features/index.html', hash: 'recording' });
});

test('asset_keeps_its_file_name', () => {
	assert.deepEqual(targetFor(`${BASE}/_astro/home.Ab12.webp`, BASE), { file: '_astro/home.Ab12.webp', hash: '' });
});

test('extensionless_path_without_slash_is_a_page', () => {
	assert.deepEqual(targetFor(`${BASE}/start/install`, BASE), { file: 'start/install/index.html', hash: '' });
});

test('same_page_hash', () => {
	assert.deepEqual(targetFor('#main', BASE), { file: '', hash: 'main' });
});

test('external_and_special_links_are_skipped', () => {
	for (const url of ['https://github.com/o/r', 'mailto:a@b.c', '//cdn.example/x.js', 'data:image/png;base64,AA']) {
		assert.equal(targetFor(url, BASE), null);
	}
});

test('root_link_outside_the_base_is_an_error', () => {
	assert.deepEqual(targetFor('/start/install/', BASE), { error: 'outside the site base' });
});

test('links_and_ids_are_collected', () => {
	const html = '<a href="/x/">x</a><img src="/y.webp" alt=""><h2 id="files">F</h2><div id=\'q\'></div>';
	assert.deepEqual(linksIn(html), ['/x/', '/y.webp']);
	assert.deepEqual([...idsIn(html)], ['files', 'q']);
});
