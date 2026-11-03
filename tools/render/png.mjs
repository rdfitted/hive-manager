import { inflateSync } from 'node:zlib';
// Playwright emits non-interlaced 8-bit RGB/RGBA PNGs. Decode pixels, not file size.
export function decodePng(bytes, retainRows = true) {
  if (!bytes.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]))) throw new Error('Screenshot is not a PNG');
  let width, height, channels;
  const chunks = [];
  for (let offset = 8; offset + 12 <= bytes.length;) {
    const length = bytes.readUInt32BE(offset), type = bytes.toString('ascii', offset + 4, offset + 8);
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    if (data.length !== length) throw new Error('Truncated PNG');
    if (type === 'IHDR') {
      width = data.readUInt32BE(0); height = data.readUInt32BE(4);
      if (data[8] !== 8 || ![2, 6].includes(data[9]) || data[12] !== 0) throw new Error('Unsupported screenshot PNG encoding');
      channels = data[9] === 2 ? 3 : 4;
    }
    if (type === 'IDAT') chunks.push(data);
    offset += length + 12;
  }
  if (!width || !height || !channels || width * height > 20000000) throw new Error('Invalid screenshot dimensions');
  const data = inflateSync(Buffer.concat(chunks), { maxOutputLength: height * (width * channels + 1) });
  const stride = width * channels;
  if (data.length !== height * (stride + 1)) throw new Error('Screenshot pixel length mismatch');
  let previous = new Uint8Array(stride);
  const colors = new Set();
  const rows = [];
  const paeth = (a, b, c) => { const p = a + b - c, x = Math.abs(p - a), y = Math.abs(p - b), z = Math.abs(p - c); return x <= y && x <= z ? a : y <= z ? b : c; };
  for (let y = 0; y < height; y++) {
    const filter = data[y * (stride + 1)], row = new Uint8Array(stride);
    if (filter > 4) throw new Error('Invalid PNG row filter');
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? row[x - channels] : 0, b = previous[x], c = x >= channels ? previous[x - channels] : 0;
      const predictor = [0, a, b, Math.floor((a + b) / 2), paeth(a, b, c)][filter];
      row[x] = (data[y * (stride + 1) + 1 + x] + predictor) & 255;
    }
    if (colors.size < 8192) for (let x = 0; x < stride; x += channels) colors.add((row[x] << 24) | (row[x + 1] << 16) | (row[x + 2] << 8) | (channels === 4 ? row[x + 3] : 255));
    previous = row;
    if (retainRows) rows.push(row);
  }
  return { width, height, channels, rows, colors: colors.size, colorCountCapped: colors.size >= 8192, hasPixels: colors.size >= 32 };
}

export function pngPixels(bytes) {
  const { rows, channels, ...summary } = decodePng(bytes, false);
  return summary;
}

// A narrow paint probe, not a reference-image comparison. Ignore the rounded
// border and compare the rows outside each maximal uniform 1-4px band.
export function badgePaint(bytes) {
  const { width, height, channels, rows } = decodePng(bytes);
  const color = (y, x) => rows[y].subarray(x * channels, (x + 1) * channels).toString();
  const uniformInk = y => {
    const ink = color(y, 2);
    for (let x = 2; x < width - 2; x++) if (color(y, x) !== ink) return null;
    return ink;
  };
  const runs = [];
  const low = Math.max(1, Math.floor(height * 0.3)), high = Math.min(height - 1, Math.ceil(height * 0.7));
  for (let y = low; y < high; y++) {
    const ink = uniformInk(y);
    if (ink === null || uniformInk(y - 1) === ink) continue;
    let end = y + 1;
    while (end < height && uniformInk(end) === ink) end++;
    if (end - y > 4 || end >= height) continue;
    let differentAbove = 0, differentBelow = 0;
    for (let x = 2; x < width - 2; x++) {
      differentAbove += Number(color(y - 1, x) !== ink);
      differentBelow += Number(color(end, x) !== ink);
    }
    if (differentAbove > (width - 4) / 4 && differentBelow > (width - 4) / 4) {
      for (let row = y; row < end; row++) runs.push(row);
    }
    y = end - 1;
  }
  return { width, height, centralHorizontalRuns: runs, clear: runs.length === 0 };
}
