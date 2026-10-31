// The dashboard's chart options, built from the server's numbers and the page's colours. Pure, so
// they are tested without a canvas. Severity colours are the app's status colours (the same as
// the map and the tower list), and every chart also names its values in text: an axis label, a
// tooltip and the panel's table, so nothing is told by colour alone.

import type { EChartsCoreOption } from "echarts/core";

import { SEVERITIES, type Severity, type TowerHealthRow } from "../api/schemas";
import type { Tokens } from "../app/colours";
import { rank } from "./model";

export type { Tokens };

/** Most bars a tally shows; the rest are folded into "Others". */
export const TALLY_BARS = 12;

export const SEVERITY_LABEL: Record<Severity, string> = {
  none: "Healthy",
  low: "Low",
  medium: "Medium",
  high: "High",
  critical: "Critical",
};

function axisStyle(t: Tokens) {
  return {
    axisLine: { lineStyle: { color: t.border } },
    axisTick: { show: false },
    axisLabel: { color: t.muted },
    splitLine: { lineStyle: { color: t.border, opacity: 0.6 } },
  };
}

function tooltip(t: Tokens) {
  return {
    backgroundColor: t.surface,
    borderColor: t.border,
    textStyle: { color: t.text },
    confine: true,
  };
}

/** Inspected towers by their worst finding. */
export function severityOption(counts: readonly number[], t: Tokens): EChartsCoreOption {
  return {
    animation: false,
    grid: { left: 8, right: 8, top: 24, bottom: 8, containLabel: true },
    tooltip: { ...tooltip(t), trigger: "item", formatter: "{b}: {c} towers" },
    xAxis: { type: "category", data: SEVERITIES.map((s) => SEVERITY_LABEL[s]), ...axisStyle(t), splitLine: { show: false } },
    yAxis: { type: "value", minInterval: 1, ...axisStyle(t) },
    series: [
      {
        type: "bar",
        barMaxWidth: 36,
        label: { show: true, position: "top", color: t.text },
        data: SEVERITIES.map((s, i) => ({ value: counts[i] ?? 0, itemStyle: { color: t.sev[s], borderRadius: [4, 4, 0, 0] } })),
      },
    ],
  };
}

/** Every inspected tower along the line: walking order across, its rating up. Click opens it. */
export function alongOption(rows: readonly TowerHealthRow[], t: Tokens, describe: (r: TowerHealthRow) => string): EChartsCoreOption {
  const shown = rows.filter((r) => r.inspected);
  return {
    animation: false,
    grid: { left: 8, right: 16, top: 16, bottom: 28, containLabel: true },
    tooltip: {
      ...tooltip(t),
      trigger: "item",
      formatter: (p: { dataIndex: number }) => {
        const r = shown[p.dataIndex];
        return r ? describe(r) : "";
      },
    },
    xAxis: {
      type: "value",
      name: "Tower in walking order",
      nameLocation: "middle",
      nameGap: 24,
      min: 0,
      minInterval: 1,
      nameTextStyle: { color: t.muted },
      ...axisStyle(t),
      splitLine: { show: false },
    },
    yAxis: { type: "category", data: SEVERITIES.map((s) => SEVERITY_LABEL[s]), ...axisStyle(t) },
    series: [
      {
        type: "scatter",
        symbolSize: 9,
        large: shown.length > 5000,
        data: shown.map((r) => ({
          value: [r.position + 1, rank(r.severity)],
          itemStyle: { color: t.sev[r.severity], borderColor: t.surface, borderWidth: 1 },
        })),
      },
    ],
  };
}

export interface Tally {
  /** The filter value a click applies; `null` for whole-tower findings or "Others". */
  key: string | null;
  label: string;
  count: number;
  worst: Severity;
}

/** The biggest `TALLY_BARS - 1` and, when there are more, one "Others" bar for the rest. */
export function foldTally(items: readonly Tally[]): Tally[] {
  if (items.length <= TALLY_BARS) return [...items];
  const head = items.slice(0, TALLY_BARS - 1);
  const rest = items.slice(TALLY_BARS - 1);
  let count = 0;
  let worst: Severity = "none";
  for (const r of rest) {
    count += r.count;
    if (rank(r.worst) > rank(worst)) worst = r.worst;
  }
  return [...head, { key: null, label: `${rest.length} others`, count, worst }];
}

/** Findings by defect or part, most first; each bar is coloured by its worst finding. */
export function tallyOption(items: readonly Tally[], t: Tokens): EChartsCoreOption {
  return {
    animation: false,
    grid: { left: 8, right: 32, top: 8, bottom: 8, containLabel: true },
    tooltip: {
      ...tooltip(t),
      trigger: "item",
      formatter: (p: { dataIndex: number }) => {
        const r = items[p.dataIndex];
        return r ? `${escapeHtml(r.label)}: ${r.count} confirmed, worst ${SEVERITY_LABEL[r.worst].toLowerCase()}` : "";
      },
    },
    xAxis: { type: "value", minInterval: 1, ...axisStyle(t) },
    yAxis: {
      type: "category",
      inverse: true,
      data: items.map((r) => r.label),
      ...axisStyle(t),
      axisLabel: { color: t.muted, width: 160, overflow: "truncate" },
      splitLine: { show: false },
    },
    series: [
      {
        type: "bar",
        barMaxWidth: 18,
        label: { show: true, position: "right", color: t.text },
        data: items.map((r) => ({ value: r.count, itemStyle: { color: t.sev[r.worst], borderRadius: [0, 4, 4, 0] } })),
      },
    ],
  };
}

/** Text for an ECharts tooltip, which is HTML. */
export function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}
