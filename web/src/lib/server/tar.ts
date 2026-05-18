/**
 * Minimal in-memory tar.gz builder. Server-side only — uses Node's zlib.
 *
 * The server's bundle validator accepts a gzipped USTAR archive containing
 * `SKILL.md` at the root and (optionally) other files alongside. For the
 * web editor we only ever publish a SKILL.md, so a single-file builder is
 * enough. No new npm deps; the math is small and well-defined.
 */

import { gzipSync } from 'node:zlib';

const BLOCK = 512;

function octal(n: number, width: number): string {
  // USTAR fields are NUL-terminated octal strings. `width` includes the NUL.
  return n.toString(8).padStart(width - 1, '0') + '\0';
}

function writeAscii(buf: Buffer, str: string, offset: number, max: number): void {
  const truncated = str.length > max - 1 ? str.slice(0, max - 1) : str;
  buf.write(truncated, offset, max - 1, 'utf-8');
  buf[offset + truncated.length] = 0;
}

function buildHeader(name: string, size: number): Buffer {
  const h = Buffer.alloc(BLOCK);
  writeAscii(h, name, 0, 100); // name
  h.write(octal(0o644, 8), 100, 8, 'utf-8'); // mode
  h.write(octal(0, 8), 108, 8, 'utf-8'); // uid
  h.write(octal(0, 8), 116, 8, 'utf-8'); // gid
  h.write(octal(size, 12), 124, 12, 'utf-8'); // size
  h.write(octal(Math.floor(Date.now() / 1000), 12), 136, 12, 'utf-8'); // mtime
  h.fill(0x20, 148, 156); // chksum placeholder (8 spaces)
  h.write('0', 156, 1, 'utf-8'); // typeflag — regular file
  h.write('ustar', 257, 5, 'utf-8'); // magic
  h.write('00', 263, 2, 'utf-8'); // version

  // Checksum: sum of all bytes treating chksum field as spaces (which we did).
  let sum = 0;
  for (let i = 0; i < BLOCK; i++) sum += h[i];
  // Field format: 6 octal digits + NUL + space.
  h.write(sum.toString(8).padStart(6, '0') + '\0 ', 148, 8, 'utf-8');
  return h;
}

export function buildSkillBundle(skillMd: string): Uint8Array {
  const data = Buffer.from(skillMd, 'utf-8');
  const header = buildHeader('SKILL.md', data.length);

  const padded = Buffer.alloc(Math.ceil(data.length / BLOCK) * BLOCK);
  data.copy(padded);

  // Two empty blocks terminate the archive.
  const trailer = Buffer.alloc(BLOCK * 2);

  const tar = Buffer.concat([header, padded, trailer]);
  return gzipSync(tar);
}
