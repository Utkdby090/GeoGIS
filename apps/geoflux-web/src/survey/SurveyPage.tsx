// /surveys/:id: one synced survey. The tower list and the map on the left and centre, the
// selected tower on the right. The selected tower is in the address (?tower=), so a link to it
// can be shared with a colleague in the same organisation.

import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useMemo, useState } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";

import { describeFailure } from "../api/http";
import type { DomainSchema, SurveyDetail, TowerCollection, TowerFeature } from "../api/schemas";
import { useApi } from "../app/api-context";
import { Loading, Notice } from "../app/ui";
import { TowerPanel } from "./TowerPanel";

const LineMap = lazy(() => import("./LineMap"));

/** Most rows the list draws at once; a search narrows the rest. Keeps a 50,000-tower line fast. */
export const LIST_LIMIT = 500;

export function SurveyPage() {
  const { id = "" } = useParams();
  const api = useApi();
  const survey = useQuery({ queryKey: ["survey", id], queryFn: ({ signal }) => api.survey(id, signal), staleTime: Infinity });
  const towers = useQuery({ queryKey: ["towers", id], queryFn: ({ signal }) => api.towers(id, signal), staleTime: Infinity });
  const schemaId = survey.data?.summary.schemaId;
  const schema = useQuery({
    queryKey: ["schema", schemaId],
    queryFn: ({ signal }) => api.schema(schemaId ?? "", signal),
    enabled: !!schemaId,
    staleTime: Infinity,
  });

  if (survey.isPending || towers.isPending) return <Loading what="Loading the survey" />;
  const error = survey.error ?? towers.error;
  if (error) {
    return (
      <main className="wide">
        <Notice kind="error">{describeFailure(error)}</Notice>
      </main>
    );
  }
  if (!survey.data || !towers.data) {
    return (
      <main className="wide">
        <Notice kind="error">This survey doesn't exist, or belongs to another organisation.</Notice>
        <Link to="/">Back to surveys</Link>
      </main>
    );
  }
  return <Survey detail={survey.data} towers={towers.data} schema={schema.data ?? null} />;
}

function Survey({ detail, towers, schema }: { detail: SurveyDetail; towers: TowerCollection; schema: DomainSchema | null }) {
  const api = useApi();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const selected = params.get("tower");
  const s = detail.summary;
  const index = useMemo(() => new Map(towers.features.map((f, i) => [f.id, i])), [towers]);
  const at = selected === null ? undefined : index.get(selected);
  const current = at === undefined ? undefined : towers.features[at];
  const select = (towerId: string | null) => setParams(towerId ? { tower: towerId } : {}, { replace: false });

  return (
    <main className="survey">
      <header className="survey-head">
        <div>
          <h1>{s.line ?? s.project}</h1>
          <p className="muted">
            {s.line ? `${s.project} · ` : ""}
            {s.assets} towers · synced {new Date((s.committed ?? s.uploaded) * 1000).toLocaleString()}
          </p>
        </div>
        {detail.revisions.length > 1 ? (
          <label className="revision">
            Upload{" "}
            <select value={s.id} onChange={(e) => void navigate(`/surveys/${encodeURIComponent(e.target.value)}`)}>
              {detail.revisions.map((r) => (
                <option key={r.id} value={r.id}>
                  r{r.revision}
                  {r.committed ? `, ${new Date(r.committed * 1000).toLocaleDateString()}` : ""}
                </option>
              ))}
            </select>
          </label>
        ) : null}
        <div className="head-actions">
          <Link className="button" to={`/surveys/${encodeURIComponent(s.id)}/health`}>
            Line health
          </Link>
          <a className="button" href={api.lineReportUrl(s.id)} download>
            All tower PDFs (ZIP)
          </a>
        </div>
      </header>
      <div className={`survey-body${current ? " with-panel" : ""}`}>
        <TowerList towers={towers} selected={selected} onSelect={select} />
        <div className="map-wrap">
          <Suspense fallback={<Loading what="Loading the map" />}>
            <LineMap towers={towers} bounds={detail.bounds} selected={selected} onSelect={select} />
          </Suspense>
        </div>
        {current && at !== undefined ? (
          <TowerPanel
            key={current.id}
            surveyId={s.id}
            towerId={current.id}
            label={current.properties.label}
            schema={schema}
            previous={towers.features[at - 1]?.id ?? null}
            next={towers.features[at + 1]?.id ?? null}
            onGo={select}
            onClose={() => select(null)}
          />
        ) : null}
      </div>
    </main>
  );
}

function TowerList({ towers, selected, onSelect }: { towers: TowerCollection; selected: string | null; onSelect: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const [flaggedOnly, setFlaggedOnly] = useState(false);
  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    return towers.features.filter(
      (f) => (!flaggedOnly || f.properties.severity !== "none") && (!q || f.properties.label.toLowerCase().includes(q)),
    );
  }, [towers, query, flaggedOnly]);
  const shown = matches.slice(0, LIST_LIMIT);
  return (
    <nav className="tower-list" aria-label="Towers">
      <input
        type="search"
        placeholder="Find a tower"
        aria-label="Find a tower"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <label className="check">
        <input type="checkbox" checked={flaggedOnly} onChange={(e) => setFlaggedOnly(e.target.checked)} /> Findings only
      </label>
      <ol>
        {shown.map((f) => (
          <TowerRow key={f.id} f={f} active={f.id === selected} onSelect={onSelect} />
        ))}
      </ol>
      {matches.length > shown.length ? (
        <p className="muted small">
          Showing {shown.length} of {matches.length}. Type a tower number to find others.
        </p>
      ) : matches.length === 0 ? (
        <p className="muted small">No towers match.</p>
      ) : null}
    </nav>
  );
}

function TowerRow({ f, active, onSelect }: { f: TowerFeature; active: boolean; onSelect: (id: string) => void }) {
  return (
    <li>
      <button type="button" className={active ? "active" : undefined} aria-current={active ? "true" : undefined} onClick={() => onSelect(f.id)}>
        <span className={`dot sev-bg-${f.properties.severity}`} aria-hidden="true" />
        <span className="label">{f.properties.label}</span>
        {f.geometry ? null : <span className="muted small" title="No position: not on the map">no position</span>}
        {f.properties.severity !== "none" ? <span className={`sev sev-${f.properties.severity} small`}>{f.properties.severity}</span> : null}
      </button>
    </li>
  );
}
