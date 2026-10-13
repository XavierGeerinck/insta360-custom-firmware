import { entries } from './manifest.mjs';

const group = (name) =>
	entries.filter((e) => e.group === name && e.source).map((e) => ({ label: e.label, slug: e.slug }));

export const sidebar = [
	{
		label: 'Getting started',
		items: ['start/overview', 'start/requirements', 'start/install', 'start/first-setup', 'start/revert', 'start/troubleshooting'],
	},
	{ label: 'Guides', items: ['features', ...group('guides')] },
	{ label: 'How it works', items: ['how/architecture', 'how/hardware'] },
	{ label: 'Hardware & protocol reference', items: group('reference') },
	{
		label: 'Research',
		items: [
			...group('research'),
			{ label: 'Device access & extraction', collapsed: true, items: group('research-access') },
			{ label: 'Analysis reports', collapsed: true, items: group('research-analysis') },
			{ label: 'Stock UI', collapsed: true, items: group('research-stock-ui') },
			{ label: 'NPU research', collapsed: true, items: [{ autogenerate: { directory: 'research/npu' } }] },
		],
	},
	{ label: 'Development', items: ['dev/building', 'dev/testing', 'dev/structure', 'dev/contributing'] },
];
