// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import mermaid from 'astro-mermaid';
import starlightLinksValidator from 'starlight-links-validator';
import { SITE, BASE, REPO_URL, BRANCH } from './site.config.mjs';
import { sidebar } from './sync/sidebar.mjs';

// Links the validator cannot check. Each entry says why.
const VALIDATOR_EXCLUDES = [
	// The front page is a custom page (src/pages/index.astro), which Starlight's
	// validator cannot see; scripts/check-dist-links.mjs checks it after the build.
	`${BASE}/`,
];

export default defineConfig({
	site: SITE,
	base: BASE,
	// mermaid's bundle is large by nature; this is its chunk, not ours.
	vite: { build: { chunkSizeWarningLimit: 2500 } },
	integrations: [
		// Must come before starlight() so it sees the Markdown first.
		mermaid(),
		starlight({
			title: 'Wave Open Firmware',
			description:
				'Open firmware for the Insta360 Wave speakerphone: no telemetry, recordings that stay yours, and a one-script way back to stock.',
			social: [{ icon: 'github', label: 'GitHub', href: REPO_URL }],
			// The Wave's own colours and typeface, shared with the front page (src/pages/index.astro).
			customCss: ['./src/styles/starlight-theme.css'],
			// Hand-written pages; synced pages carry their own editUrl.
			editLink: { baseUrl: `${REPO_URL}/edit/${BRANCH}/site/` },
			lastUpdated: false,
			// src/content/docs/404.md is our 404 page. Starlight's own /404 route would
			// collide with it (a build warning), so the docs route renders it instead.
			disable404Route: true,
			plugins: [starlightLinksValidator({ exclude: VALIDATOR_EXCLUDES })],
			sidebar,
		}),
	],
});
