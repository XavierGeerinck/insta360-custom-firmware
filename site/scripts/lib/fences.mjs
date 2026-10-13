const FENCE = /^\s{0,3}(`{3,}|~{3,})/;

// Tracks Markdown fenced code blocks (``` or ~~~, up to 3 leading spaces) line by line.
// Returns a function that, called on each line in order, returns true when the line
// is a fence delimiter or inside a fenced block (i.e. not ordinary Markdown text).
export function createFenceTracker() {
	let fence = null;
	return function isFenced(line) {
		const f = line.match(FENCE);
		if (fence) {
			if (f && f[1][0] === fence[0] && f[1].length >= fence.length) fence = null;
			return true;
		}
		if (f) {
			fence = f[1];
			return true;
		}
		return false;
	};
}
