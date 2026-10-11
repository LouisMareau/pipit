// The session log: everything playing together does (hosting, joining, the
// start, delay changes, late keys and rollbacks, digest checks, stalls, the
// end) plus a status line a second, printed to the browser console under
// "[pipit link]" and kept here for "Copy session log". Meant to be pasted into
// a bug report when a game misbehaves only online.

const MAX_ENTRIES = 4000;
const entries: string[] = [];
const started = performance.now();

/** Appends a line (prefixed with the seconds since the page loaded) and prints it. */
export function linkLog(text: string) {
  const line = `${((performance.now() - started) / 1000).toFixed(2)}s ${text}`;
  entries.push(line);
  if (entries.length > MAX_ENTRIES) entries.shift();
  console.info(`[pipit link] ${line}`);
}

/** The whole log so far, one line per entry. */
export function linkLogText(): string {
  return entries.join("\n");
}
