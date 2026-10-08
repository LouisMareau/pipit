// Save the current frame as a PNG.

import type { Screen } from "../ui/screen";

export async function takeScreenshot(screen: Screen, title: string) {
  const blob = await screen.toBlob();
  if (!blob) return;
  const url = URL.createObjectURL(blob);
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const a = document.createElement("a");
  a.href = url;
  a.download = `${title || "pipit"}-${stamp}.png`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
