// Where each part's marker goes on the 3D tower, without three.js: a part sits at the model node
// its schema entry names, a part the model lacks sits at the tower's centre (and says so), and
// findings on no part sit above the tower.

import type { PartDef, PartHealth, Severity } from "../api/schemas";

export type Vec3 = readonly [number, number, number];

/** The key for findings that name no part; never a valid part id, which can't be empty. */
export const WHOLE_TOWER = "";

export interface Spot {
  /** The part id, or WHOLE_TOWER. */
  key: string;
  at: Vec3;
  severity: Severity;
  counted: number;
  pending: number;
  /** False when the model has no node for the part, so the marker sits at the centre. */
  placed: boolean;
}

/** The node a part's markers sit at: its `node` or, when empty, its id. */
export function nodeName(part: string, defs: readonly PartDef[]): string {
  const d = defs.find((p) => p.id === part);
  return d?.node || part;
}

export function spots(
  parts: readonly PartHealth[],
  defs: readonly PartDef[],
  nodes: ReadonlyMap<string, Vec3>,
  centre: Vec3,
  above: Vec3,
): Spot[] {
  return parts.map((p) => {
    const base = { severity: p.severity, counted: p.counted, pending: p.pending };
    if (p.part === null) return { key: WHOLE_TOWER, at: above, placed: true, ...base };
    const at = nodes.get(nodeName(p.part, defs));
    return at ? { key: p.part, at, placed: true, ...base } : { key: p.part, at: centre, placed: false, ...base };
  });
}
