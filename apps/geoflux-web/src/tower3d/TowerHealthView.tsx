// A tower's health in the tower panel: the 3D tower with a marker per part that has findings,
// the same parts as a list (the accessible way in), and the findings of the picked part.
// Rated by confirmed findings only; proposals are listed as waiting.

import { useQuery } from "@tanstack/react-query";
import { Component, lazy, Suspense, useMemo, useState, type ReactNode } from "react";

import { describeFailure } from "../api/http";
import type { DomainSchema, Finding, PartHealth } from "../api/schemas";
import { useApi } from "../app/api-context";
import { useColours } from "../app/colours";
import { Loading, Notice } from "../app/ui";
import { SEVERITY_LABEL } from "../health/charts";
import { Labels, measure } from "../health/model";
import { WHOLE_TOWER } from "./spots";

const TowerModel = lazy(() => import("./TowerModel"));

export function TowerHealthView({ surveyId, towerId, schema }: { surveyId: string; towerId: string; schema: DomainSchema | null }) {
  const api = useApi();
  const health = useQuery({
    queryKey: ["tower-health", surveyId, towerId],
    queryFn: ({ signal }) => api.towerHealth(surveyId, towerId, signal),
    staleTime: Infinity,
  });
  const [picked, setPicked] = useState<string | null>(null);
  const [show3d, setShow3d] = useState(true);
  const [failed, setFailed] = useState<string | null>(null);
  const colours = useColours();
  const labels = useMemo(() => new Labels(schema), [schema]);

  if (health.isPending) return <Loading what="Loading the tower's health" />;
  if (health.error) return <Notice kind="error">{describeFailure(health.error)}</Notice>;
  const h = health.data;
  if (!h || h.parts.length === 0) {
    return (
      <section className="tower-health" aria-label="Tower health">
        <h3>Tower health</h3>
        <p className="muted small">No findings were synced for this tower.</p>
      </section>
    );
  }
  const keyOf = (p: PartHealth) => p.part ?? WHOLE_TOWER;
  const partOf = (f: Finding) => f.part ?? WHOLE_TOWER;
  const shown = picked === null ? h.findings : h.findings.filter((f) => partOf(f) === picked);
  const missing = new Set(h.parts.filter((p) => p.part !== null && !(schema?.parts ?? []).some((d) => d.id === p.part)).map(keyOf));
  const toggle = (key: string) => setPicked((k) => (k === key ? null : key));

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
          <p className="muted small">The 3D tower can't be drawn in this browser. The parts are listed below.</p>
        ) : (
          <ModelBoundary onError={setFailed}>
            <Suspense fallback={<Loading what="Loading the 3D tower" />}>
              <TowerModel
                parts={h.parts}
                defs={schema?.parts ?? []}
                selected={picked}
                onPick={toggle}
                colours={colours.sev}
                onFail={setFailed}
              />
            </Suspense>
          </ModelBoundary>
        )
      ) : null}
      <ul className="part-list" aria-label="Parts with findings">
        {h.parts.map((p) => {
          const key = keyOf(p);
          return (
            <li key={key}>
              <button type="button" aria-pressed={picked === key} onClick={() => toggle(key)}>
                <span className={`dot sev-bg-${p.counted > 0 ? p.severity : "none"}`} aria-hidden="true" />
                <span className="label">{labels.part(p.part)}</span>
                {p.counted > 0 ? <span className={`sev sev-${p.severity} small`}>{SEVERITY_LABEL[p.severity]}</span> : null}
                <span className="muted small">
                  {p.counted > 0 ? `${p.counted} confirmed` : ""}
                  {p.counted > 0 && p.pending > 0 ? ", " : ""}
                  {p.pending > 0 ? `${p.pending} waiting` : ""}
                </span>
              </button>
              {missing.has(key) ? <span className="muted small"> (not on the 3D model; shown at its centre)</span> : null}
            </li>
          );
        })}
      </ul>
      <h4 className="small">
        {picked === null ? "All findings" : `Findings on: ${labels.part(picked === WHOLE_TOWER ? null : picked)}`}
        {picked !== null ? (
          <>
            {" "}
            <button type="button" className="link small" onClick={() => setPicked(null)}>
              Show all
            </button>
          </>
        ) : null}
      </h4>
      <ol className="finding-list">
        {shown.map((f) => (
          <li key={f.id}>
            <div>
              <span className={`sev sev-${f.severity}`}>{SEVERITY_LABEL[f.severity]}</span> {labels.defect(f.defect)}
              {f.status === "proposed" ? <span className="tag">Waiting for review</span> : null}
            </div>
            <div className="muted small">
              {labels.part(f.part)}
              {Object.entries(f.measurements ?? {}).map(([k, v]) => {
                const m = measure(k);
                return ` · ${m.label} ${Number.isInteger(v) ? v : v.toFixed(1)}${m.unit ? ` ${m.unit}` : ""}`;
              })}
            </div>
            {f.note ? <div className="small note">{f.note}</div> : null}
          </li>
        ))}
      </ol>
    </section>
  );
}

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
