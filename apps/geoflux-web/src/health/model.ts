// What the dashboard shows, worked out from the server's LineHealth and the survey's schema, with
// no React: the filters in the address, the filter choices, labels, and the towers that need
// attention. Kept pure so it is tested without a browser.

import { SEVERITIES, type DomainSchema, type HealthQuery, type LineHealth, type Severity, type TowerHealthRow } from "../api/schemas";

/** Most rows the attention table draws; the rest are counted below it. */
export const ATTENTION_LIMIT = 200;

/** A dotted id: lowercase words joined by dots, the same rule as the server's. */
const DOTTED = /^[a-z0-9_]+(\.[a-z0-9_]+)*$/;

/** The filters from the address (`?minSeverity=high&defect=hotspot&part=insulator`). Anything
 * that can't be a filter is dropped, so an edited link shows the line rather than an error. */
export function filtersFrom(params: URLSearchParams): HealthQuery {
  const q: HealthQuery = {};
  const min = params.get("minSeverity");
  if (min && min !== "none" && (SEVERITIES as readonly string[]).includes(min)) q.minSeverity = min as Severity;
  const defect = params.get("defect");
  if (defect && defect.length <= 64 && DOTTED.test(defect)) q.defect = defect;
  const part = params.get("part");
  if (part && part.length <= 64 && DOTTED.test(part)) q.part = part;
  return q;
}

/** The address's query for `q`, in a fixed order so one view has one link. */
export function filtersTo(q: HealthQuery): Record<string, string> {
  const out: Record<string, string> = {};
  if (q.minSeverity) out.minSeverity = q.minSeverity;
  if (q.defect) out.defect = q.defect;
  if (q.part) out.part = q.part;
  return out;
}

export interface Choice {
  value: string;
  label: string;
}

/** `insulator.left.r` → `Insulator`; `bird_nest` → `Bird nest`. */
function familyLabel(id: string): string {
  const words = id.replaceAll("_", " ").replaceAll(".", " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

/** Each family with more than one member first ("Insulator, all"), then every id with its label. */
function choices(defs: readonly { id: string; label: string }[]): Choice[] {
  const families = new Map<string, number>();
  for (const d of defs) {
    const dot = d.id.indexOf(".");
    if (dot > 0) {
      const f = d.id.slice(0, dot);
      families.set(f, (families.get(f) ?? 0) + 1);
    }
  }
  const out: Choice[] = [];
  for (const [f, n] of families) if (n > 1 && !defs.some((d) => d.id === f)) out.push({ value: f, label: `${familyLabel(f)}, all` });
  for (const d of defs) out.push({ value: d.id, label: d.label });
  return out;
}

export function defectChoices(schema: DomainSchema | null): Choice[] {
  return choices(schema?.defects ?? []);
}

export function partChoices(schema: DomainSchema | null): Choice[] {
  return choices(schema?.parts ?? []);
}

/** Labels for defect and part ids; an id the schema no longer lists shows as itself. */
export class Labels {
  private readonly defects: Map<string, string>;
  private readonly parts: Map<string, string>;

  constructor(schema: DomainSchema | null) {
    this.defects = new Map((schema?.defects ?? []).map((d) => [d.id, d.label]));
    this.parts = new Map((schema?.parts ?? []).map((p) => [p.id, p.label]));
  }

  defect(id: string): string {
    return this.defects.get(id) ?? id;
  }

  /** `null` is a finding on the whole tower. */
  part(id: string | null | undefined): string {
    if (id === null || id === undefined) return "Whole tower";
    return this.parts.get(id) ?? id;
  }
}

export function rank(s: Severity): number {
  return SEVERITIES.indexOf(s);
}

/** Towers with a counted finding, worst first, then in walking order; at most `limit`. */
export function attention(rows: readonly TowerHealthRow[], limit = ATTENTION_LIMIT): { rows: TowerHealthRow[]; total: number } {
  const flagged = rows.filter((r) => r.severity !== "none");
  flagged.sort((a, b) => rank(b.severity) - rank(a.severity) || a.position - b.position);
  return { rows: flagged.slice(0, limit), total: flagged.length };
}

/** The headline numbers. `act` is towers rated high or critical. */
export function tiles(h: LineHealth): { towers: number; inspected: number; act: number; findings: number; pending: number } {
  const c = h.severityCounts;
  return { towers: h.towers, inspected: h.inspected, act: (c[3] ?? 0) + (c[4] ?? 0), findings: h.findings, pending: h.pending };
}

/** How a measurement reads on the dashboard: `riseC` → "Hottest rise" in °C. */
const MEASURES: Record<string, { label: string; unit: string }> = {
  riseC: { label: "Hottest rise", unit: "°C" },
  maxC: { label: "Hottest spot", unit: "°C" },
  minC: { label: "Coldest spot", unit: "°C" },
  meanC: { label: "Hottest mean", unit: "°C" },
};

export function measure(name: string): { label: string; unit: string } {
  return MEASURES[name] ?? { label: name, unit: "" };
}
