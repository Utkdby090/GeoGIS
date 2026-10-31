// Writes src/tower3d/tower.glb: the one static tower the web app shows for every tower, a 765 kV
// double-circuit suspension tower drawn as lattice lines, with an empty node per tower part for
// markers to sit at. Run `node scripts/make-tower-model.mjs` after changing it; the file is
// checked in, so builds don't run this.
//
// Node names are the parts' ids in the built-in schema (crates/geoflux-domain/schemas/telecom.json).
// A better model (bought, from Blender) can replace this file as long as it keeps those names.
// Units are metres, y up, the line runs along z; "left" is -x looking along the line.

import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const out = join(here, "../src/tower3d/tower.glb");

const H = 64; // top of the earthwire peaks
const BASE = 7; // half the base width
const WAIST_Y = 30; // where the body stops tapering
const WAIST = 2.2; // half the width at the waist and above
const ARMS = [
  { y: 44, reach: 13, phase: "r" },
  { y: 38, reach: 15, phase: "y" },
  { y: 32, reach: 13, phase: "b" },
];
const STRING = 6; // insulator string length

const steel = [];
const insulators = [];
const seg = (list, a, b) => list.push(...a, ...b);

/** Half width of the body at height y. */
const half = (y) => (y >= WAIST_Y ? WAIST : BASE + ((WAIST - BASE) * y) / WAIST_Y);
const corners = (y) => {
  const w = half(y);
  return [
    [-w, y, -w],
    [w, y, -w],
    [w, y, w],
    [-w, y, w],
  ];
};

// The body: corner legs, rings at each panel, and X bracing on each face.
const levels = [0, 6, 12, 18, 24, 30, 32, 35, 38, 41, 44, 48, 52];
for (let i = 0; i < levels.length; i++) {
  const c = corners(levels[i]);
  for (let k = 0; k < 4; k++) seg(steel, c[k], c[(k + 1) % 4]);
  if (i === 0) continue;
  const p = corners(levels[i - 1]);
  for (let k = 0; k < 4; k++) {
    seg(steel, p[k], c[k]);
    seg(steel, p[k], c[(k + 1) % 4]);
    seg(steel, p[(k + 1) % 4], c[k]);
  }
}

// The earthwire peaks: two masts from the top of the body, leaning out.
const top = 52;
const peaks = { left: [-9, H, 0], right: [9, H, 0] };
for (const [side, tip] of Object.entries(peaks)) {
  const s = side === "left" ? -1 : 1;
  seg(steel, [s * WAIST, top, -WAIST], tip);
  seg(steel, [s * WAIST, top, WAIST], tip);
  seg(steel, [-s * WAIST, top, 0], tip);
}

// Cross-arms both sides: a triangle in plan, from the body face to the tip, with an insulator
// string hanging from each tip.
const nodes = [];
const node = (name, t) => nodes.push({ name, translation: t.map((v) => Math.round(v * 100) / 100) });
for (const side of ["left", "right"]) {
  const s = side === "left" ? -1 : 1;
  for (const arm of ARMS) {
    const tip = [s * arm.reach, arm.y, 0];
    const under = [s * WAIST, arm.y - 3, 0];
    seg(steel, [s * WAIST, arm.y, -WAIST], tip);
    seg(steel, [s * WAIST, arm.y, WAIST], tip);
    seg(steel, [s * WAIST, arm.y, -WAIST], under);
    seg(steel, [s * WAIST, arm.y, WAIST], under);
    seg(steel, under, tip);
    for (let k = 1; k < 4; k++) {
      const x = s * (WAIST + ((arm.reach - WAIST) * k) / 4);
      const zz = WAIST * (1 - k / 4);
      seg(steel, [x, arm.y, -zz], [x, arm.y, zz]);
    }
    const end = [tip[0], arm.y - STRING, 0];
    seg(insulators, tip, end);
    // A short conductor stub along the line, so the string reads as holding something.
    seg(insulators, [end[0], end[1], -3], [end[0], end[1], 3]);
    node(`insulator.${side}.${arm.phase}`, [tip[0], arm.y - STRING / 2, 0]);
  }
  node(`earthwire.${side}`, peaks[side]);
}
node("body", [0, 22, 0]);
node("plate", [0, 3, -half(3)]); // number and danger plates face the line
const feet = corners(0);
["a", "b", "c", "d"].forEach((leg, k) => node(`leg.${leg}`, [feet[k][0], 1, feet[k][2]]));

// ---- glTF 2.0 binary ----------------------------------------------------------------------
function accessorFor(list) {
  const f = new Float32Array(list);
  const min = [Infinity, Infinity, Infinity];
  const max = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < f.length; i += 3) {
    for (let a = 0; a < 3; a++) {
      min[a] = Math.min(min[a], f[i + a]);
      max[a] = Math.max(max[a], f[i + a]);
    }
  }
  return { f, min, max };
}
const parts = [accessorFor(steel), accessorFor(insulators)];
const buffers = [];
let offset = 0;
const bufferViews = [];
const accessors = [];
for (const p of parts) {
  bufferViews.push({ buffer: 0, byteOffset: offset, byteLength: p.f.byteLength, target: 34962 });
  accessors.push({ bufferView: bufferViews.length - 1, componentType: 5126, count: p.f.length / 3, type: "VEC3", min: p.min, max: p.max });
  buffers.push(Buffer.from(p.f.buffer));
  offset += p.f.byteLength;
}
const bin = Buffer.concat(buffers);
const gltf = {
  asset: { version: "2.0", generator: "GeoFlux make-tower-model.mjs" },
  scene: 0,
  scenes: [{ name: "Tower", nodes: [0] }],
  nodes: [
    { name: "tower", children: [1, 2, ...nodes.map((_, i) => 3 + i)] },
    { name: "steel", mesh: 0 },
    { name: "insulators", mesh: 1 },
    ...nodes,
  ],
  meshes: [
    { name: "steel", primitives: [{ attributes: { POSITION: 0 }, mode: 1, material: 0 }] },
    { name: "insulators", primitives: [{ attributes: { POSITION: 1 }, mode: 1, material: 1 }] },
  ],
  materials: [
    { name: "steel", pbrMetallicRoughness: { baseColorFactor: [0.55, 0.58, 0.62, 1], metallicFactor: 0.6, roughnessFactor: 0.5 } },
    { name: "insulator", pbrMetallicRoughness: { baseColorFactor: [0.35, 0.55, 0.75, 1], metallicFactor: 0, roughnessFactor: 0.6 } },
  ],
  accessors,
  bufferViews,
  buffers: [{ byteLength: bin.length }],
};
const pad = (b, fill) => (b.length % 4 ? Buffer.concat([b, Buffer.alloc(4 - (b.length % 4), fill)]) : b);
const json = pad(Buffer.from(JSON.stringify(gltf)), 0x20);
const body = pad(bin, 0);
const header = Buffer.alloc(12);
header.writeUInt32LE(0x46546c67, 0);
header.writeUInt32LE(2, 4);
header.writeUInt32LE(12 + 8 + json.length + 8 + body.length, 8);
const chunk = (type, data) => {
  const h = Buffer.alloc(8);
  h.writeUInt32LE(data.length, 0);
  h.writeUInt32LE(type, 4);
  return Buffer.concat([h, data]);
};
writeFileSync(out, Buffer.concat([header, chunk(0x4e4f534a, json), chunk(0x004e4942, body)]));
console.log(`wrote ${out}: ${steel.length / 6} steel members, ${nodes.length} part nodes`);
