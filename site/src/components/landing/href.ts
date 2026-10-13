// Site-internal links for pages outside Starlight: they need the base path
// (/insta360-custom-firmware) that Starlight adds to its own pages.
const base = import.meta.env.BASE_URL.replace(/\/$/, '');

export const href = (path: string) => `${base}/${path.replace(/^\//, '')}`;
