// The surveys GeoGIS has synced, newest first. The map and tower pages build on this list.
// Every Sync is a new upload (revision) of the same survey; the list shows each survey once, at its
// newest upload, and the survey page lists the earlier ones.

import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { Link } from "react-router";

import { describeFailure } from "../api/http";
import { SEVERITIES, type SurveySummary } from "../api/schemas";
import { useApi } from "../app/api-context";
import { Loading, Notice } from "../app/ui";

export function SurveysPage() {
  const api = useApi();
  const surveys = useQuery({ queryKey: ["surveys"], queryFn: ({ signal }) => api.surveys(signal) });
  const latest = useMemo(() => newestUploads(surveys.data ?? []), [surveys.data]);
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
        <table className="list surveys">
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
            {latest.map(({ survey: s, uploads }) => (
              <tr key={s.id}>
                <td>
                  <Link to={`/surveys/${encodeURIComponent(s.id)}`}>{s.line ?? s.project}</Link>{" "}
                  <span className="muted">{uploads > 1 ? `r${s.revision} · ${uploads} uploads` : `r${s.revision}`}</span>
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

/** Each survey once (by GeoGIS's survey id), at its newest upload, in the list's order. */
export function newestUploads(list: readonly SurveySummary[]): { survey: SurveySummary; uploads: number }[] {
  const by = new Map<string, { survey: SurveySummary; uploads: number }>();
  for (const s of list) {
    const seen = by.get(s.clientSurveyId);
    if (!seen) {
      by.set(s.clientSurveyId, { survey: s, uploads: 1 });
    } else {
      seen.uploads += 1;
      if (s.revision > seen.survey.revision) seen.survey = s;
    }
  }
  return [...by.values()];
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
