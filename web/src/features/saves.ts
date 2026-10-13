// Export and import battery saves as .sav files, compatible with other emulators.

export function exportSave(data: ArrayBuffer, name: string) {
  const url = URL.createObjectURL(new Blob([data]));
  const a = document.createElement("a");
  a.href = url;
  a.download = `${name}.sav`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

export function pickSaveFile(): Promise<ArrayBuffer | null> {
  return pickFile(".sav,.sa1,.srm");
}

/** Lets the player choose a file; null when they cancel. */
export function pickFile(accept: string): Promise<ArrayBuffer | null> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = accept;
    input.addEventListener("change", async () => {
      const file = input.files?.[0];
      resolve(file ? await file.arrayBuffer() : null);
    });
    input.addEventListener("cancel", () => resolve(null));
    input.click();
  });
}
