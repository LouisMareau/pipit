// A small PNG decoder for the scripts: 8-bit greyscale, RGB and RGBA, no
// interlacing (what the emulator's screenshots and browsers write).
import { inflateSync } from "node:zlib";

export const PNG = {
  /** Decodes `buf` to `{ width, height, data }`, `data` being RGBA bytes. */
  decode(buf) {
    let pos = 8;
    let width = 0;
    let height = 0;
    let colorType = 0;
    const idat = [];
    while (pos < buf.length) {
      const len = buf.readUInt32BE(pos);
      const type = buf.toString("ascii", pos + 4, pos + 8);
      const data = buf.subarray(pos + 8, pos + 8 + len);
      if (type === "IHDR") {
        width = data.readUInt32BE(0);
        height = data.readUInt32BE(4);
        colorType = data[9];
        if (data[8] !== 8) throw new Error("only 8-bit PNGs are supported");
        if (data[12] !== 0) throw new Error("interlaced PNGs are not supported");
      } else if (type === "IDAT") idat.push(data);
      else if (type === "IEND") break;
      pos += 12 + len;
    }
    const channels = { 0: 1, 2: 3, 4: 2, 6: 4 }[colorType];
    if (!channels) throw new Error(`unsupported PNG colour type ${colorType}`);
    const raw = inflateSync(Buffer.concat(idat));
    const stride = width * channels;
    const out = Buffer.alloc(width * height * 4);
    let prev = Buffer.alloc(stride);
    let cur = Buffer.alloc(stride);
    for (let y = 0; y < height; y++) {
      const filter = raw[y * (stride + 1)];
      const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
      for (let i = 0; i < stride; i++) {
        const a = i >= channels ? cur[i - channels] : 0;
        const b = prev[i];
        const c = i >= channels ? prev[i - channels] : 0;
        const x = line[i];
        let v;
        switch (filter) {
          case 0: v = x; break;
          case 1: v = x + a; break;
          case 2: v = x + b; break;
          case 3: v = x + ((a + b) >> 1); break;
          case 4: {
            const p = a + b - c;
            const pa = Math.abs(p - a);
            const pb = Math.abs(p - b);
            const pc = Math.abs(p - c);
            v = x + (pa <= pb && pa <= pc ? a : pb <= pc ? b : c);
            break;
          }
          default: throw new Error(`bad PNG filter ${filter}`);
        }
        cur[i] = v & 0xff;
      }
      for (let x = 0; x < width; x++) {
        const o = (y * width + x) * 4;
        const s = x * channels;
        if (channels >= 3) {
          out[o] = cur[s];
          out[o + 1] = cur[s + 1];
          out[o + 2] = cur[s + 2];
          out[o + 3] = channels === 4 ? cur[s + 3] : 255;
        } else {
          out[o] = out[o + 1] = out[o + 2] = cur[s];
          out[o + 3] = channels === 2 ? cur[s + 1] : 255;
        }
      }
      [prev, cur] = [cur, prev];
    }
    return { width, height, data: out };
  },
};
