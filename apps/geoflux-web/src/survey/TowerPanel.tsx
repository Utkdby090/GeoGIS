// One tower: its form as GeoGIS uploaded it, its photos with markup, its PDF, and the towers
// before and after it in walking order.

import { useQuery } from "@tanstack/react-query";
import { useState } from "react";

import { describeFailure } from "../api/http";
import type { DomainSchema, PhotoRef, TowerDetail } from "../api/schemas";
import { useApi } from "../app/api-context";
import { Loading, Notice } from "../app/ui";
import { markupElements } from "./markup";

export interface TowerPanelProps {
  surveyId: string;
  towerId: string;
  label: string;
  schema: DomainSchema | null;
  previous: string | null;
  next: string | null;
  onGo: (towerId: string) => void;
  onClose: () => void;
}

export function TowerPanel({ surveyId, towerId, label, schema, previous, next, onGo, onClose }: TowerPanelProps) {
  const api = useApi();
  const tower = useQuery({
    queryKey: ["tower", surveyId, towerId],
    queryFn: ({ signal }) => api.tower(surveyId, towerId, signal),
    // An upload never changes, so a tower read once is good for the session.
    staleTime: Infinity,
  });
  return (
    <section className="tower-panel" aria-label={`Tower ${label}`}>
      <header className="tower-head">
        <button type="button" disabled={!previous} onClick={() => previous && onGo(previous)} aria-label="Previous tower">
          ‹
        </button>
        <h2>Tower {label}</h2>
        <button type="button" disabled={!next} onClick={() => next && onGo(next)} aria-label="Next tower">
          ›
        </button>
        <button type="button" className="close" onClick={onClose} aria-label="Close">
          ×
        </button>
      </header>
      {tower.isPending ? (
        <Loading />
      ) : tower.error ? (
        <Notice kind="error">{describeFailure(tower.error)}</Notice>
      ) : !tower.data ? (
        <Notice kind="error">This tower isn't in the survey.</Notice>
      ) : (
        <TowerBody detail={tower.data} schema={schema} reportUrl={api.towerReportUrl(surveyId, towerId)} label={label} />
      )}
    </section>
  );
}

function TowerBody({ detail, schema, reportUrl, label }: { detail: TowerDetail; schema: DomainSchema | null; reportUrl: string; label: string }) {
  const values = new Map(detail.asset.properties);
  const loc = detail.asset.location;
  const value = (key: string): string | undefined => {
    if (key === "core.latitude") return loc ? String(loc.lat) : undefined;
    if (key === "core.longitude") return loc ? String(loc.lon) : undefined;
    return values.get(key);
  };
  const groups = groupFields(schema, [...values.keys()]);
  const slots = schema?.imageSlots ?? [...new Set(detail.photos.map((p) => p.slot))].map((id) => ({ id, label: id, thermal: false }));
  return (
    <div className="tower-body">
      <p className="tower-meta">
        <span className={`sev sev-${detail.asset.severity}`}>{detail.asset.severity === "none" ? "No finding" : detail.asset.severity}</span>
        <a className="button" href={reportUrl} download>
          Tower PDF
        </a>
      </p>
      <div className="photos">
        {slots.map((slot) => {
          const photo = detail.photos.find((p) => p.slot === slot.id);
          return <PhotoCell key={slot.id} title={slot.label} photo={photo ?? null} surveyId={detail.survey} tower={label} />;
        })}
      </div>
      {groups.map(([group, keys]) => (
        <section key={group} className="field-group">
          <h3>{group}</h3>
          <dl>
            {keys.map(({ key, label: fieldLabel }) => (
              <div key={key} className="row">
                <dt>{fieldLabel}</dt>
                <dd>{value(key)?.trim() || <span className="muted">N/A</span>}</dd>
              </div>
            ))}
          </dl>
        </section>
      ))}
    </div>
  );
}

/** The schema's fields by group, in the schema's order, then any uploaded values it doesn't know. */
export function groupFields(schema: DomainSchema | null, uploadedKeys: readonly string[]): [string, { key: string; label: string }[]][] {
  const groups = new Map<string, { key: string; label: string }[]>();
  const known = new Set<string>();
  for (const f of schema?.fields ?? []) {
    if (f.type === "photo") continue;
    known.add(f.key);
    const list = groups.get(f.group) ?? [];
    list.push({ key: f.key, label: f.label });
    groups.set(f.group, list);
  }
  const other = uploadedKeys.filter((k) => !known.has(k)).map((k) => ({ key: k, label: k }));
  if (other.length) groups.set("Other", other);
  return [...groups];
}

function PhotoCell({ title, photo, surveyId, tower }: { title: string; photo: PhotoRef | null; surveyId: string; tower: string }) {
  const api = useApi();
  const [size, setSize] = useState<{ w: number; h: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [showMarkup, setShowMarkup] = useState(true);
  if (!photo) {
    return (
      <figure className="photo empty">
        <div className="frame">No image</div>
        <figcaption>{title}</figcaption>
      </figure>
    );
  }
  const url = api.photoUrl(surveyId, photo.sha256);
  const markup = photo.markup ?? [];
  return (
    <figure className="photo">
      <a className="frame" href={url} target="_blank" rel="noopener">
        {failed ? (
          <span className="muted">Couldn't load this photo.</span>
        ) : (
          <>
            <img
              src={url}
              alt={`${title}, tower ${tower}`}
              loading="lazy"
              decoding="async"
              onLoad={(e) => setSize({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
              onError={() => setFailed(true)}
            />
            {size && showMarkup && markup.length > 0 ? (
              <svg viewBox={`0 0 ${size.w} ${size.h}`} preserveAspectRatio="none" aria-hidden="true">
                {markupElements(markup, size.w, size.h)}
              </svg>
            ) : null}
          </>
        )}
      </a>
      <figcaption>
        {title}
        {markup.length > 0 ? (
          <label className="markup-toggle">
            <input type="checkbox" checked={showMarkup} onChange={(e) => setShowMarkup(e.target.checked)} /> Markup
          </label>
        ) : null}
      </figcaption>
    </figure>
  );
}
