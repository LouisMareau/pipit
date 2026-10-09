// Inline SVG icons for the toolbar and dialogs. All draw with `currentColor`, so
// a button's active/inactive colours apply to them unchanged.

const PATHS = {
  back: '<path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/>',
  pause: '<rect x="6" y="5" width="4.5" height="14" rx="1.2"/><rect x="13.5" y="5" width="4.5" height="14" rx="1.2"/>',
  fastForward:
    '<path d="M3 6.4v11.2c0 .9 1 1.5 1.8 1l8-5.6c.7-.5.7-1.5 0-2l-8-5.6c-.8-.5-1.8.1-1.8 1z"/>' +
    '<path d="M12 6.4v11.2c0 .9 1 1.5 1.8 1l8-5.6c.7-.5.7-1.5 0-2l-8-5.6c-.8-.5-1.8.1-1.8 1z"/>',
  camera:
    '<path fill-rule="evenodd" d="M9 4.5h6l1.4 2.5H20a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V9a2 2 0 0 1 2-2h3.6L9 4.5z' +
    'M12 9.2a3.8 3.8 0 1 0 0 7.6 3.8 3.8 0 0 0 0-7.6z"/><circle cx="12" cy="13" r="2.2"/>',
  more: '<circle cx="5" cy="12" r="2.1"/><circle cx="12" cy="12" r="2.1"/><circle cx="19" cy="12" r="2.1"/>',
  close: '<path d="M6 6l12 12M18 6L6 18" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>',
} as const;

export type IconName = keyof typeof PATHS;

export function icon(name: IconName, size = 20): string {
  return `<svg viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true" fill="currentColor">${PATHS[name]}</svg>`;
}

/** A chevron pointing up, down, left or right (for the touch D-pad). */
export function chevron(direction: "up" | "down" | "left" | "right", size = 18): string {
  const rotate = { up: 90, down: -90, left: 0, right: 180 }[direction];
  return (
    `<svg viewBox="0 0 24 24" width="${size}" height="${size}" aria-hidden="true" style="transform: rotate(${rotate}deg)">` +
    '<path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"/></svg>'
  );
}
