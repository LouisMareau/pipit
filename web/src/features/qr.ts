// A QR code as an SVG, for handing a transfer code to a phone's camera.

import qrcode from "qrcode-generator";

/** `text` as a QR code; `label` is what screen readers get. */
export function qrSvg(text: string, label: string): string {
  const qr = qrcode(0, "M");
  qr.addData(text);
  qr.make();
  const n = qr.getModuleCount();
  let path = "";
  for (let row = 0; row < n; row++) {
    for (let col = 0; col < n; col++) if (qr.isDark(row, col)) path += `M${col} ${row}h1v1h-1z`;
  }
  const size = n + 4;
  return (
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="-2 -2 ${size} ${size}" shape-rendering="crispEdges" role="img" aria-label="${label}">` +
    `<rect x="-2" y="-2" width="${size}" height="${size}" fill="#fff"/><path d="${path}" fill="#000"/></svg>`
  );
}
