// A tower's health: the 3D tower with a numbered marker at each part that has findings, and the
// same findings as a numbered list (the accessible way in). Picking a finding, a marker or a badge
// highlights that part and moves the camera to it. Rated by confirmed findings only; proposals are
// listed as waiting and shown as hollow rings.

import { useQuery } from "@tanstack/react-query";
import { Component, lazy, Suspense, useMemo, useState, type ReactNode } from "react";

import { describeFailure } from "../api/http";
import type { DomainSchema, Finding, PartHealth } from "../api/schemas";
import { useApi } from "../app/api-context";
import { useColours } from "../app/colours";
import { Loading, Notice } from "../app/ui";
import { SEVERITY_LABEL } from "../health/charts";
import { Labels, measure } from "../health/model";
import { inkOn } from "../survey/ratings";
import type { Badge } from "./TowerModel";
import { WHOLE_TOWER } from "./spots";

const TowerModel = lazy(() => import("./TowerModel"));

export interface TowerHealthViewProps {
  surveyId: string;
  towerId: string;
  schema: DomainSchema | null;
  /** Start with the tower turning (unless the person asked for less motion). */
  turnAtStart?: boolean;
}

export function TowerHealthView({ surveyId, towerId, schema, turnAtStart = false }: TowerHealthViewProps) {
  const api = useApi();
  const health = useQuery({
    queryKey: ["tower-health", surveyId, towerId],
    queryFn: ({ signal }) => api.towerHealth(surveyId, towerId, signal),
    staleTime: Infinity,
  });
  const [picked, setPicked] = useState<string | null>(null);
  const [show3d, setShow3d] = useState(true);
  const [failed, setFailed] = useState<string | null>(null);
  const [allParts, setAllParts] = useState(false);
  const [turn, setTurn] = useState(turnAtStart);
  const [home, setHome] = useState(0);
  const colours = useColours();
  const labels = useMemo(() => new Labels(schema), [schema]);
  const h = health.data;

  // Confirmed findings come first from the server, worst first; each gets the number its badge shows.
  const { counted, waiting, badges } = useMemo(() => {
    const findings = h?.findings ?? [];
    const counted = findings.filter((f) => f.status !== "proposed");
    const waiting = findings.filter((f) => f.status === "proposed");
    const numbers = new Map<string, number[]>();
    counted.forEach((f, i) => {
      const k = partOf(f);
      numbers.set(k, [...(numbers.get(k) ?? []), i + 1]);
    });
    const badges = new Map<string, Badge>();
    for (const p of h?.parts ?? []) {
      const key = keyOf(p);
      const n = numbers.get(key);
      if (!n || p.counted === 0) continue;
      const colour = colours.sev[p.severity];
      badges.set(key, {
        text: n.join(","),
        label: `${n.length > 1 ? "Findings" : "Finding"} ${n.join(", ")}: ${labels.part(p.part)}, ${SEVERITY_LABEL[p.severity].toLowerCase()}`,
        colour,
        ink: inkOn(colour),
      });
    }
    return { counted, waiting, badges };
  }, [h, colours, labels]);

  if (health.isPending) return <Loading what="Loading the tower's health" />;
  if (health.error) return <Notice kind="error">{describeFailure(health.error)}</Notice>;
  if (!h) {
    return (
      <section className="tower-health" aria-label="Tower health">
        <h3>Tower health</h3>
        <p className="muted small">No findings were synced for this tower.</p>
      </section>
    );
  }
  // No findings yet: the tower still shows, unmarked, so it's clear where they will appear.
  const empty = h.parts.length === 0;
  const missing = new Set(h.parts.filter((p) => p.part !== null && !(schema?.parts ?? []).some((d) => d.id === p.part)).map(keyOf));
  const toggle = (key: string) => setPicked((k) => (k === key ? null : key));
  const reset = () => {
    setPicked(null);
    setHome((n) => n + 1);
  };

  const item = (f: Finding, n: number | null) => {
    const key = partOf(f);
    const colour = colours.sev[f.severity];
    return (
      <li key={f.id}>
        <button type="button" className="fitem" aria-pressed={picked === key} onClick={() => toggle(key)}>
          {n === null ? (
            <span className="num pending" aria-hidden="true" />
          ) : (
            <span className="num" style={{ background: colour, color: inkOn(colour) }}>
              {n}
            </span>
          )}
          <span className="fbody">
            <b>{labels.defect(f.defect)}</b>
            <span>
              {labels.part(f.part ?? null)}
              {missing.has(key) ? " (not on the 3D model; shown at its centre)" : ""}
            </span>
            {f.measurements && Object.keys(f.measurements).length > 0 ? (
              <span>
                {Object.entries(f.measurements)
                  .map(([k, v]) => {
                    const m = measure(k);
                    return `${m.label} ${Number.isInteger(v) ? v : v.toFixed(1)}${m.unit ? ` ${m.unit}` : ""}`;
                  })
                  .join(" · ")}
              </span>
            ) : null}
            {f.note ? <span className="note">{f.note}</span> : null}
          </span>
          {n === null ? <span className="tag">Waiting for review</span> : <span className={`sev sev-${f.severity}`}>{SEVERITY_LABEL[f.severity]}</span>}
        </button>
      </li>
    );
  };

  return (
    <section className="tower-health" aria-label="Tower health">
      <h3>
        Tower health{" "}
        <button type="button" className="link small" aria-pressed={show3d} onClick={() => setShow3d((v) => !v)}>
          {show3d ? "Hide 3D" : "Show 3D"}
        </button>
      </h3>
      {show3d ? (
        failed ? (
          <p className="muted small">The 3D tower can't be drawn in this browser. The findings are listed below.</p>
        ) : (
          <div className="tower-stage">
            <ModelBoundary onError={setFailed}>
              <Suspense fallback={<Loading what="Loading the 3D tower" />}>
                <TowerModel
                  parts={h.parts}
                  defs={schema?.parts ?? []}
                  selected={picked}
                  onPick={toggle}
                  colours={colours.sev}
                  quiet={colours.unknown}
                  badges={badges}
                  allParts={allParts}
                  turn={turn}
                  onTurnStop={() => setTurn(false)}
                  home={home}
                  onFail={setFailed}
                />
              </Suspense>
            </ModelBoundary>
            <div className="stage-tools" role="group" aria-label="3D view">
              <button type="button" onClick={reset}>
                Reset view
              </button>
              <label>
                <input type="checkbox" checked={allParts} onChange={(e) => setAllParts(e.target.checked)} /> All parts
              </label>
              <label>
                <input type="checkbox" checked={turn} onChange={(e) => setTurn(e.target.checked)} /> Turn
              </label>
            </div>
            <p className="stage-help">Drag to turn · scroll to zoom</p>
          </div>
        )
      ) : null}
      {empty ? (
        <p className="muted small">
          No findings yet. Findings made in GeoGIS (Analyze › Inspection › Review Findings, with a part picked) and
          synced show here on their part of the tower.
        </p>
      ) : (
        <>
          <h4 className="small">
            Confirmed findings on {h.label} ({counted.length})
          </h4>
          {counted.length === 0 ? (
            <p className="muted small">None yet: every finding on this tower is waiting for review.</p>
          ) : (
            <ol className="finding-list" aria-label="Confirmed findings">
              {counted.map((f, i) => item(f, i + 1))}
            </ol>
          )}
          {waiting.length > 0 ? (
            <>
              <h4 className="small">Waiting for review ({waiting.length})</h4>
              <ul className="finding-list" aria-label="Findings waiting for review">
                {waiting.map((f) => item(f, null))}
              </ul>
            </>
          ) : null}
        </>
      )}
    </section>
  );
}

const keyOf = (p: PartHealth) => p.part ?? WHOLE_TOWER;
const partOf = (f: Finding) => f.part ?? WHOLE_TOWER;

/** Keeps a 3D failure (a lost GPU, a bad model file) inside the panel instead of the page. */
class ModelBoundary extends Component<{ onError: (why: string) => void; children: ReactNode }, { failed: boolean }> {
  override state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  override componentDidCatch(e: unknown) {
    this.props.onError(e instanceof Error ? e.message : String(e));
  }
  override render() {
    return this.state.failed ? null : this.props.children;
  }
}
