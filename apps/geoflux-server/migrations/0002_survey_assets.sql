-- Phase 7b: every uploaded tower, kept per survey revision exactly as GeoGIS sent it, so the web
-- map can show it and Phase 7c can compare revisions over time. Never updated after the upload.
CREATE TABLE survey_assets (
    survey    text NOT NULL REFERENCES surveys (id) ON DELETE CASCADE,
    -- The tower's id in GeoGIS (client text, at most 200 characters, checked before storing).
    asset_id  text NOT NULL,
    position  bigint NOT NULL,
    -- From geoflux_domain::Severity, for the dashboard's counts without reading `body`.
    severity  text NOT NULL,
    -- NULL when the tower has no position; then it is listed but not drawn.
    lon       double precision,
    lat       double precision,
    -- The whole SurveyAsset JSON (form values, photos by hash, markup).
    body      jsonb NOT NULL,
    PRIMARY KEY (survey, asset_id)
);
CREATE INDEX survey_assets_order ON survey_assets (survey, position, asset_id);
