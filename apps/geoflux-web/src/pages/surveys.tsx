// The surveys GeoGIS has synced, newest first. The map and tower pages build on this list.

import { useQuery } from "@tanstack/react-query";

import { describeFailure } from "../api/http";
import { SEVERITIES, type SurveySummary } from "../api/schemas";
import { useApi } from "../app/api-context";
import { Loading, Notice } from "../app/ui";

export function SurveysPage() {
  const api = useApi();
  const surveys = useQuery({ queryKey: ["surveys"], queryFn: ({ signal }) => api.surveys(signal) });
  return (
    <main className="wide">
      <h1>Surveys</h1>
      {surveys.isPending ? (
        <Loading />
      ) : surveys.error ? (
        <Notice kind="error">{describeFailure(surveys.error)}</Notice>
      ) : surveys.data.length === 0 ? (
        <p className="empty">
          No surveys yet. In GeoGIS, open a survey and use Analyze › Inspection › Sync to Cloud; it appears here once
          the upload finishes.
        </p>
      ) : (
        <table className="list">
          <thead>
            <tr>
              <th scope="col">Line</th>
              <th scope="col">Project</th>
              <th scope="col">Towers</th>
              <th scope="col">Findings</th>
              <th scope="col">Synced</th>
            </tr>
          </thead>
          <tbody>
            {surveys.data.map((s) => (
              <tr key={s.id}>
                <td>
                  {s.line ?? "—"} <span className="muted">r{s.revision}</span>
                </td>
                <td>{s.project}</td>
                <td>{s.assets}</td>
                <td>
                  <Findings survey={s} />
                </td>
                <td>{new Date((s.committed ?? s.uploaded) * 1000).toLocaleString()}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </main>
  );
}

function Findings({ survey }: { survey: SurveySummary }) {
  const parts = SEVERITIES.map((sev, i) => [sev, survey.severityCounts[i] ?? 0] as const).filter(
    ([sev, n]) => sev !== "none" && n > 0,
  );
  if (parts.length === 0) return <span className="muted">None</span>;
  return (
    <span className="severities">
      {parts.reverse().map(([sev, n]) => (
        <span key={sev} className={`sev sev-${sev}`} title={sev}>
          {n} {sev}
        </span>
      ))}
    </span>
  );
}
