// The dashboard's 3D tower: a 765 kV double-circuit suspension tower built from its members in
// code (legs, bracing, three cross-arms a side, earthwire peaks, glass-disc insulator strings,
// quad-bundle conductors, foundations and a number plate), as on the dashboard sample Utkarsh
// picked. The survey page keeps the lighter tower.glb.
//
// Members are instanced cylinders, so the whole tower is a handful of draw calls. Every part a
// finding can name has an empty object (a node) at the place its marker goes, named by its part
// id, the same names tower.glb uses, so markers find their place the same way on both models.
// Building it allocates once; nothing here runs per frame.

import * as THREE from "three";

import type { Vec3 } from "./spots";

/** Where each part's marker sits (metres: x across the line, left circuit negative; y up; z
 * along the line, +z facing the first view). R is the top phase, then Y, then B. */
export const LATTICE_PARTS: Readonly<Record<string, Vec3>> = {
  "earthwire.left": [-6, 68, 0],
  "earthwire.right": [6, 68, 0],
  "insulator.left.r": [-13, 55, 0],
  "insulator.right.r": [13, 55, 0],
  "insulator.left.y": [-16, 46, 0],
  "insulator.right.y": [16, 46, 0],
  "insulator.left.b": [-13, 37, 0],
  "insulator.right.b": [13, 37, 0],
  body: [0, 24, 4.4],
  plate: [0, 3.5, 7.2],
  "leg.a": [-7.5, 1, 7.5],
  "leg.b": [7.5, 1, 7.5],
  "leg.c": [7.5, 1, -7.5],
  "leg.d": [-7.5, 1, -7.5],
};

/** The colours the tower is drawn in, from the page's CSS (light or dark). */
export interface LatticeColours {
  steel: string;
  insulator: string;
  wire: string;
  grid: string;
}

export interface LatticeTower {
  root: THREE.Group;
  nodes: Map<string, Vec3>;
  recolour: (c: LatticeColours) => void;
}

type Seg = [THREE.Vector3, THREE.Vector3];

const v = (x: number, y: number, z: number) => new THREE.Vector3(x, y, z);

/** Half the body's width at height y: tapering to the waist at 36 m, then nearly straight. */
const halfW = (y: number) => (y <= 36 ? 7.5 - 5 * (y / 36) : y <= 62 ? 2.5 - 0.5 * ((y - 36) / 26) : 2);
const corners = (y: number) => {
  const w = halfW(y);
  return [v(-w, y, w), v(w, y, w), v(w, y, -w), v(-w, y, -w)] as const;
};

/** Main members (legs, chords) and lighter bracing, the hanging points and the conductors. */
function members(): { legs: Seg[]; bracing: Seg[]; tips: THREE.Vector3[] } {
  const legs: Seg[] = [];
  const bracing: Seg[] = [];
  const levels = [0, 6, 12, 18, 24, 30, 36, 40, 44, 49, 53, 58, 62];
  for (let i = 0; i < levels.length - 1; i++) {
    const a = corners(levels[i] ?? 0);
    const b = corners(levels[i + 1] ?? 0);
    for (let c = 0; c < 4; c++) {
      const n = (c + 1) % 4;
      legs.push([a[c]!, b[c]!]);
      bracing.push([a[c]!, b[n]!], [a[n]!, b[c]!]);
    }
  }
  for (const y of levels) {
    const c = corners(y);
    for (let k = 0; k < 4; k++) bracing.push([c[k]!, c[(k + 1) % 4]!]);
  }
  for (const y of [36, 40, 49, 58, 62]) {
    const c = corners(y);
    bracing.push([c[0], c[2]], [c[1], c[3]]);
  }
  // The peak and the earthwire arms.
  const top = corners(62);
  const apex = [v(-1, 70, 1), v(1, 70, 1), v(1, 70, -1), v(-1, 70, -1)] as const;
  for (let c = 0; c < 4; c++) {
    legs.push([top[c]!, apex[c]!]);
    bracing.push([apex[c]!, apex[(c + 1) % 4]!], [top[c]!, apex[(c + 1) % 4]!]);
  }
  for (const s of [-1, 1]) {
    const tip = v(6 * s, 68, 0);
    bracing.push([v(s, 70, 1), tip], [v(s, 70, -1), tip], [v(s * 2, 66, 2), tip], [v(s * 2, 66, -2), tip]);
  }
  // Cross-arms: level bottom chords, top chords sloping to the tip, laced between.
  const tips: THREE.Vector3[] = [];
  for (const { y, reach } of [
    { y: 58, reach: 13 },
    { y: 49, reach: 16 },
    { y: 40, reach: 13 },
  ]) {
    for (const s of [-1, 1]) {
      const w = halfW(y);
      const tip = v(reach * s, y, 0);
      const x = (t: number) => s * (w + (reach - w) * t);
      const b1 = (t: number) => v(x(t), y, w * (1 - t));
      const b2 = (t: number) => v(x(t), y, -w * (1 - t));
      const t1 = (t: number) => v(x(t), y + 3.2 * (1 - t), w * (1 - t));
      const t2 = (t: number) => v(x(t), y + 3.2 * (1 - t), -w * (1 - t));
      legs.push([b1(0), tip], [b2(0), tip], [t1(0), tip], [t2(0), tip]);
      const panels = 5;
      for (let i = 0; i < panels; i++) {
        const t = i / panels;
        const u = (i + 1) / panels;
        bracing.push([b1(t), b2(t)], [t1(t), t2(t)], [b1(t), t1(t)], [b2(t), t2(t)], [b1(t), t1(u)], [b2(t), t2(u)], [b1(t), b2(u)], [t1(t), t2(u)]);
      }
      tips.push(tip);
    }
  }
  return { legs, bracing, tips };
}

/** Builds the tower. Its geometries and materials are released by disposing the scene it joins. */
export function buildLatticeTower(colours: LatticeColours): LatticeTower {
  const root = new THREE.Group();
  root.name = "lattice-tower";
  const steel = new THREE.MeshStandardMaterial({ metalness: 0.55, roughness: 0.45 });
  const glass = new THREE.MeshStandardMaterial({ metalness: 0.1, roughness: 0.25, transparent: true, opacity: 0.92 });
  const wire = new THREE.MeshStandardMaterial({ metalness: 0.6, roughness: 0.5 });
  const pad = new THREE.MeshStandardMaterial({ color: 0x9a9a92, roughness: 0.9 });
  const white = new THREE.MeshStandardMaterial({ color: 0xf2f2ee });
  const cylinder = new THREE.CylinderGeometry(1, 1, 1, 6, 1);

  // Reused for every instance.
  const up = v(0, 1, 0);
  const m = new THREE.Matrix4();
  const q = new THREE.Quaternion();
  const scale = new THREE.Vector3();
  const dir = new THREE.Vector3();
  const mid = new THREE.Vector3();
  const rods = (list: Seg[], radius: number, material: THREE.Material) => {
    const mesh = new THREE.InstancedMesh(cylinder, material, list.length);
    list.forEach(([a, b], i) => {
      dir.subVectors(b, a);
      const length = dir.length();
      q.setFromUnitVectors(up, dir.normalize());
      scale.set(radius, length, radius);
      m.compose(mid.addVectors(a, b).multiplyScalar(0.5), q, scale);
      mesh.setMatrixAt(i, m);
    });
    root.add(mesh);
  };

  const { legs, bracing, tips } = members();
  rods(legs, 0.16, steel);
  rods(bracing, 0.07, steel);

  // Insulator strings of 24 glass discs, their hardware, and a quad-bundle conductor each.
  const DISCS = 24;
  const discs = new THREE.InstancedMesh(new THREE.CylinderGeometry(0.34, 0.34, 0.11, 14), glass, tips.length * DISCS);
  const hardware: Seg[] = [];
  const conductors: Seg[] = [];
  tips.forEach((tip, k) => {
    hardware.push([tip, v(tip.x, tip.y - 6, 0)]);
    for (let i = 0; i < DISCS; i++) discs.setMatrixAt(k * DISCS + i, m.makeTranslation(tip.x, tip.y - 0.5 - i * 0.22, 0));
    for (const [dx, dy] of [
      [-0.23, -0.23],
      [0.23, -0.23],
      [-0.23, 0.23],
      [0.23, 0.23],
    ] as const) {
      conductors.push([v(tip.x + dx, tip.y - 6.2 + dy, -34), v(tip.x + dx, tip.y - 6.2 + dy, 34)]);
    }
  });
  for (const s of [-1, 1]) conductors.push([v(6 * s, 68, -34), v(6 * s, 68, 34)]);
  root.add(discs);
  rods(hardware, 0.05, steel);
  rods(conductors, 0.045, wire);

  // The ground, the foundations and the number plate on the front face.
  const grid = new THREE.GridHelper(120, 24);
  grid.position.y = -0.02;
  root.add(grid);
  const padShape = new THREE.BoxGeometry(1.6, 0.8, 1.6);
  for (const [x, z] of [
    [-7.5, 7.5],
    [7.5, 7.5],
    [7.5, -7.5],
    [-7.5, -7.5],
  ] as const) {
    const p = new THREE.Mesh(padShape, pad);
    p.position.set(x, 0.4, z);
    root.add(p);
  }
  const plate = new THREE.Mesh(new THREE.BoxGeometry(1.4, 0.9, 0.05), white);
  plate.position.set(0, 3.5, halfW(3.5) + 0.05);
  root.add(plate);

  // A named node per part, where its marker goes.
  const nodes = new Map<string, Vec3>();
  for (const [name, at] of Object.entries(LATTICE_PARTS)) {
    const o = new THREE.Object3D();
    o.name = name;
    o.position.set(...at);
    root.add(o);
    nodes.set(name, at);
  }

  const gridMaterial = grid.material as THREE.LineBasicMaterial;
  const recolour = (c: LatticeColours) => {
    steel.color.set(c.steel);
    glass.color.set(c.insulator);
    wire.color.set(c.wire);
    gridMaterial.vertexColors = false;
    gridMaterial.color.set(c.grid);
    gridMaterial.needsUpdate = true;
  };
  recolour(colours);
  return { root, nodes, recolour };
}

/** The tower's colours from the page's CSS, with the sample's light colours as the fallback. */
export function latticeColours(): LatticeColours {
  const css = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    steel: read("--tower-steel", "#8d979b"),
    insulator: read("--tower-insulator", "#5f8b84"),
    wire: read("--tower-wire", "#3d4441"),
    grid: read("--tower-grid", "#c3ccc7"),
  };
}
