// /surveys/:id/health: the line at a glance. Filters in one row at the top (kept in the address,
// so a filtered view can be shared), headline numbers, charts, and the towers that need attention.
// Panels can be dragged by their title and resized on a wide screen; the arrangement is kept in
// this browser. Every chart has its numbers in a table too.

import { keepPreviousData, useQuery } from "@tanstack/react-query";
import type { EChartsCoreOption } from "echarts/core";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject } from "react";
import { Responsive, type Layout } from "react-grid-layout";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import "react-grid-layout/css/styles.css";

import { describeFailure } from "../api/http";
import { SEVERITIES, type DomainSchema, type LineHealth, type Severity, type TowerHealthRow } from "../api/schemas";
import { useApi } from "../app/api-context";
import { useColours } from "../app/colours";
import { Loading, Notice } from "../app/ui";
import { alongOption, escapeHtml as esc, foldTally, severityOption, SEVERITY_LABEL, tallyOption, type Tally } from "./charts";
import { BREAKPOINTS, COLS, defaultLayouts, forgetWide, loadWide, ROW_HEIGHT, saveWide, type Panel } from "./layout";
import { attention, defectChoices, filtersFrom, filtersTo, Labels, measure, partChoices, tiles, type Choice } from "./model";

const EChart = lazy(() => import("./EChart"));

export function HealthPage() {
  const { id = "" } = useParams();
  const api = useApi();
  const [params, setParams] = useSearchParams();
  const query = useMemo(() => filtersFrom(params), [params]);
  const survey = useQuery({ queryKey: ["survey", id], queryFn: ({ signal }) => api.survey(id, signal), staleTime: Infinity });
  const schemaId = survey.data?.summary.schemaId;
  const schema = useQuery({
    queryKey: ["schema", schemaId],
    queryFn: ({ signal }) => api.schema(schemaId ?? "", signal),
    enabled: !!schemaId,
    staleTime: Infinity,
  });
  const health = useQuery({
    queryKey: ["health", id, filtersTo(query)],
    queryFn: ({ signal }) => api.lineHealth(id, query, signal),
    // A synced survey never changes; changing a filter keeps the last view until the next arrives.
    staleTime: Infinity,
    placeholderData: keepPreviousData,
  });

  if (survey.isPending || (health.isPending && !health.data)) return <Loading what="Loading the line's health" />;
  if (survey.error) return <Failed error={survey.error} />;
  if (!survey.data) {
    return (
      <main className="wide">
        <Notice kind="error">This survey doesn't exist, or belongs to another organisation.</Notice>
        <Link to="/">Back to surveys</Link>
      </main>
    );
  }
  const s = survey.data.summary;
  const setQuery = (next: Record<string, string>) => setParams(next, { replace: true });
  return (
    <main className="health">
      <header className="survey-head">
        <div>
          <h1>{s.line ?? s.project}: line health</h1>
          <p className="muted">
            {s.line ? `${s.project} · ` : ""}upload r{s.revision} · rated by confirmed findings only
          </p>
        </div>
        <Link className="button" to={`/surveys/${encodeURIComponent(s.id)}`}>
          Map and towers
        </Link>
      </header>
      <Filters schema={schema.data ?? null} query={filtersTo(query)} onChange={setQuery} busy={health.isFetching} />
      {health.error ? (
        <div className="health-body">
          <Notice kind="error">{describeFailure(health.error)}</Notice>
        </div>
      ) : health.data ? (
        <Dashboard
          surveyId={s.id}
          health={health.data}
          schema={schema.data ?? null}
          onFilter={(k, v) => setQuery({ ...filtersTo(query), [k]: v })}
        />
      ) : (
        <div className="health-body">
          <Notice kind="error">This survey doesn't exist, or belongs to another organisation.</Notice>
        </div>
      )}
    </main>
  );
}

function Failed({ error }: { error: unknown }) {
  return (
    <main className="wide">
      <Notice kind="error">{describeFailure(error)}</Notice>
    </main>
  );
}

function Filters({
  schema,
  query,
  onChange,
  busy,
}: {
  schema: DomainSchema | null;
  query: Record<string, string>;
  onChange: (q: Record<string, string>) => void;
  busy: boolean;
}) {
  const set = (key: string, value: string) => {
    const next = { ...query };
    if (value) next[key] = value;
    else delete next[key];
    onChange(next);
  };
  const defects = useMemo(() => defectChoices(schema), [schema]);
  const parts = useMemo(() => partChoices(schema), [schema]);
  return (
    <form className="filters" aria-label="Filters" onSubmit={(e) => e.preventDefault()}>
      <label>
        Severity{" "}
        <select value={query.minSeverity ?? ""} onChange={(e) => set("minSeverity", e.target.value)}>
          <option value="">Any</option>
          {SEVERITIES.filter((s) => s !== "none").map((s) => (
            <option key={s} value={s}>
              {SEVERITY_LABEL[s]}
              {s === "critical" ? "" : " or worse"}
            </option>
          ))}
        </select>
      </label>
      <FilterSelect label="Defect" value={query.defect ?? ""} choices={defects} onChange={(v) => set("defect", v)} />
      <FilterSelect label="Part" value={query.part ?? ""} choices={parts} onChange={(v) => set("part", v)} />
      {Object.keys(query).length > 0 ? (
        <button type="button" className="link" onClick={() => onChange({})}>
          Clear filters
        </button>
      ) : null}
      {busy ? (
        <span className="muted small" role="status">
          Updating…
        </span>
      ) : null}
    </form>
  );
}

function FilterSelect({
  label,
  value,
  choices,
  onChange,
}: {
  label: string;
  value: string;
  choices: Choice[];
  onChange: (v: string) => void;
}) {
  // A value from a shared link that this schema doesn't list is still shown, as itself.
  const known = value === "" || choices.some((c) => c.value === value);
  return (
    <label>
      {label}{" "}
      <select value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Any</option>
        {known ? null : <option value={value}>{value}</option>}
        {choices.map((c) => (
          <option key={c.value} value={c.value}>
            {c.label}
          </option>
        ))}
      </select>
    </label>
  );
}

/** The grid's width, followed as the window or sidebar changes. `null` until measured, so the
 * grid is first laid out at its real width rather than animating from a guess. */
function useWidth(): [RefObject<HTMLDivElement | null>, number | null] {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState<number | null>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => {
      // An element that isn't laid out (a test page) reports 0: use the window's width then.
      const w = el.clientWidth || window.innerWidth;
      setWidth((old) => (old === w ? old : w));
    };
    measure();
    if (typeof ResizeObserver === "undefined") {
      window.addEventListener("resize", measure);
      return () => window.removeEventListener("resize", measure);
    }
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

function Dashboard({
  surveyId,
  health,
  schema,
  onFilter,
}: {
  surveyId: string;
  health: LineHealth;
  schema: DomainSchema | null;
  onFilter: (key: "defect" | "part", value: string) => void;
}) {
  const navigate = useNavigate();
  const tokens = useColours();
  const [ref, width] = useWidth();
  const [layouts, setLayouts] = useState(() => ({ ...defaultLayouts(), lg: loadWide() }));
  const wide = width !== null && width >= BREAKPOINTS.lg;
  const labels = useMemo(() => new Labels(schema), [schema]);
  const filtered = Object.keys(health.query).length > 0;
  const towerLink = useCallback((id: string) => `/surveys/${encodeURIComponent(surveyId)}?tower=${encodeURIComponent(id)}`, [surveyId]);
  const labelOf = useMemo(() => new Map(health.rows.map((r) => [r.id, r.label])), [health.rows]);

  const describe = useCallback(
    (r: TowerHealthRow) => {
      const top = r.top ? `<br>${esc(labels.defect(r.top.defect))}, ${esc(labels.part(r.top.part))}` : "";
      const counts = r.pending > 0 ? `<br>${r.findings} confirmed, ${r.pending} waiting` : `<br>${r.findings} confirmed`;
      return `Tower ${esc(r.label)}: ${SEVERITY_LABEL[r.severity].toLowerCase()}${top}${counts}`;
    },
    [labels],
  );
  const defects: Tally[] = useMemo(
    () => foldTally(health.byDefect.map((d) => ({ key: d.defect, label: labels.defect(d.defect), count: d.count, worst: d.worst }))),
    [health.byDefect, labels],
  );
  const parts: Tally[] = useMemo(
    () => foldTally(health.byPart.map((p) => ({ key: p.part, label: labels.part(p.part), count: p.count, worst: p.worst }))),
    [health.byPart, labels],
  );
  const inspected = useMemo(() => health.rows.filter((r) => r.inspected), [health.rows]);
  const options = useMemo(
    () => ({
      severity: severityOption(health.severityCounts, tokens),
      along: alongOption(health.rows, tokens, describe),
      defects: tallyOption(defects, tokens),
      parts: tallyOption(parts, tokens),
    }),
    [health, tokens, describe, defects, parts],
  );
  const t = tiles(health);
  const need = attention(health.rows);

  const panels: Record<Panel, { title: string; body: ReactNode }> = {
    tiles: {
      title: filtered ? "At a glance, matching the filters" : "At a glance",
      body: (
        <dl className="tiles">
          <Tile label="Towers" value={t.towers} />
          <Tile label="Inspected" value={t.inspected} />
          <Tile label="Need action (high or critical)" value={t.act} tone={t.act > 0 ? "critical" : undefined} />
          <Tile label="Confirmed findings" value={t.findings} />
          <Tile label="Waiting for review" value={t.pending} />
        </dl>
      ),
    },
    severity: {
      title: "Towers by worst finding",
      body: (
        <ChartWithTable
          option={options.severity}
          label="Inspected towers by their worst finding"
          head={["Rating", "Towers"]}
          rows={SEVERITIES.map((s, i) => [SEVERITY_LABEL[s], String(health.severityCounts[i] ?? 0)])}
        />
      ),
    },
    along: {
      title: "Along the line",
      body:
        inspected.length === 0 ? (
          <p className="empty">No tower of this upload was inspected.</p>
        ) : (
          <ChartWithTable
            option={options.along}
            label="Each inspected tower's rating in walking order; select a dot to open the tower"
            onPick={(_s, i) => {
              const r = inspected[i];
              if (r) void navigate(towerLink(r.id));
            }}
            head={["Tower", "Rating"]}
            rows={inspected.filter((r) => r.severity !== "none").map((r) => [r.label, SEVERITY_LABEL[r.severity]])}
          />
        ),
    },
    defects: {
      title: "Findings by defect",
      body:
        defects.length === 0 ? (
          <p className="empty">No confirmed findings{filtered ? " match the filters" : ""}.</p>
        ) : (
          <ChartWithTable
            option={options.defects}
            label="Confirmed findings by defect type; select a bar to filter by it"
            onPick={(_s, i) => {
              const k = defects[i]?.key;
              if (k) onFilter("defect", k);
            }}
            head={["Defect", "Confirmed", "Worst"]}
            rows={defects.map((d) => [d.label, String(d.count), SEVERITY_LABEL[d.worst]])}
          />
        ),
    },
    parts: {
      title: "Findings by tower part",
      body:
        parts.length === 0 ? (
          <p className="empty">No confirmed findings{filtered ? " match the filters" : ""}.</p>
        ) : (
          <ChartWithTable
            option={options.parts}
            label="Confirmed findings by tower part; select a bar to filter by it"
            onPick={(_s, i) => {
              const k = parts[i]?.key;
              if (k) onFilter("part", k);
            }}
            head={["Part", "Confirmed", "Worst"]}
            rows={parts.map((p) => [p.label, String(p.count), SEVERITY_LABEL[p.worst]])}
          />
        ),
    },
    attention: {
      title: `Towers needing attention (${need.total})`,
      body:
        need.total === 0 ? (
          <p className="empty">No tower has a confirmed finding{filtered ? " that matches the filters" : ""}.</p>
        ) : (
          <>
            <table className="list compact">
              <thead>
                <tr>
                  <th>Tower</th>
                  <th>Rating</th>
                  <th>Worst finding</th>
                  <th>Confirmed</th>
                  <th>Waiting</th>
                </tr>
              </thead>
              <tbody>
                {need.rows.map((r) => (
                  <tr key={r.id}>
                    <td>
                      <Link to={towerLink(r.id)}>{r.label}</Link>
                    </td>
                    <td>
                      <SeverityTag severity={r.severity} />
                    </td>
                    <td>{r.top ? `${labels.defect(r.top.defect)}, ${labels.part(r.top.part)}` : ""}</td>
                    <td>{r.findings}</td>
                    <td>{r.pending || ""}</td>
                  </tr>
                ))}
              </tbody>
            </table>
            {need.total > need.rows.length ? (
              <p className="muted small">
                Showing the worst {need.rows.length} of {need.total}. Filter to narrow the list.
              </p>
            ) : null}
          </>
        ),
    },
    peaks: {
      title: "Highest readings",
      body:
        health.peaks.length === 0 ? (
          <p className="empty">No confirmed finding has a measurement{filtered ? " that matches the filters" : ""}.</p>
        ) : (
          <table className="list compact">
            <thead>
              <tr>
                <th>Reading</th>
                <th>Value</th>
                <th>Tower</th>
              </tr>
            </thead>
            <tbody>
              {health.peaks.map((p) => {
                const m = measure(p.measure);
                return (
                  <tr key={p.measure}>
                    <td>{m.label}</td>
                    <td>
                      {Number.isInteger(p.value) ? p.value : p.value.toFixed(1)}
                      {m.unit ? ` ${m.unit}` : ""}
                    </td>
                    <td>
                      <Link to={towerLink(p.tower)}>{labelOf.get(p.tower) ?? p.tower}</Link>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        ),
    },
  };

  const onLayoutChange = (_current: Layout, all: Partial<Record<string, Layout>>) => {
    if (!wide || !all.lg) return;
    setLayouts((l) => ({ ...l, lg: all.lg ?? l.lg }));
    saveWide(all.lg);
  };
  const reset = () => {
    forgetWide();
    setLayouts(defaultLayouts());
  };

  return (
    <div className="health-body">
      {wide ? (
        <p className="muted small grid-hint">
          Drag a panel by its title or resize it from its corner.{" "}
          <button type="button" className="link" onClick={reset}>
            Reset layout
          </button>
        </p>
      ) : null}
      <div ref={ref}>
        {width === null ? null : (
          <Responsive
            width={width}
            layouts={layouts}
            breakpoints={BREAKPOINTS}
            cols={COLS}
            rowHeight={ROW_HEIGHT}
            margin={[12, 12]}
            containerPadding={[0, 0]}
            dragConfig={{ enabled: wide, handle: ".panel-head", allowMobileScroll: true }}
            resizeConfig={{ enabled: wide }}
            onLayoutChange={onLayoutChange}
          >
            {(Object.keys(panels) as Panel[]).map((key) => (
              <section key={key} className="panel" aria-labelledby={`panel-${key}`}>
                <header className="panel-head">
                  <h2 id={`panel-${key}`}>{panels[key].title}</h2>
                </header>
                <div className="panel-body">{panels[key].body}</div>
              </section>
            ))}
          </Responsive>
        )}
      </div>
    </div>
  );
}

function Tile({ label, value, tone }: { label: string; value: number; tone?: Severity | undefined }) {
  return (
    <div className="tile">
      <dt>{label}</dt>
      <dd className={tone ? `sev-${tone}` : undefined}>{value.toLocaleString()}</dd>
    </div>
  );
}

function SeverityTag({ severity }: { severity: Severity }) {
  return (
    <span className="sev-tag">
      <span className={`dot sev-bg-${severity}`} aria-hidden="true" />
      <span className={`sev sev-${severity}`}>{SEVERITY_LABEL[severity]}</span>
    </span>
  );
}

function ChartWithTable({
  option,
  label,
  onPick,
  head,
  rows,
}: {
  option: EChartsCoreOption;
  label: string;
  onPick?: (series: number, index: number) => void;
  head: string[];
  rows: string[][];
}) {
  const [asTable, setAsTable] = useState(false);
  return (
    <div className="chart-box">
      <button type="button" className="link small table-toggle" aria-pressed={asTable} onClick={() => setAsTable((v) => !v)}>
        {asTable ? "Show chart" : "Show table"}
      </button>
      {asTable ? (
        <table className="list compact">
          <thead>
            <tr>
              {head.map((h) => (
                <th key={h}>{h}</th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((r, i) => (
              <tr key={i}>
                {r.map((c, j) => (
                  <td key={j}>{c}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      ) : (
        <Suspense fallback={<Loading what="Drawing the chart" />}>
          <EChart option={option} label={label} {...(onPick ? { onPick } : {})} />
        </Suspense>
      )}
    </div>
  );
}
