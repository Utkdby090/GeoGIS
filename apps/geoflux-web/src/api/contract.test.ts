// The web app's schemas against the JSON the Rust types really produce (fixtures written by
// crates/geoflux-cloud/tests/web_fixtures.rs). If this fails after a Rust change, fix schemas.ts.

import { describe, expect, it } from "vitest";
import type { z } from "zod";

import { ApiFailure } from "./client";
import { path, ROUTES } from "./routes";
import * as S from "./schemas";

import devices from "./fixtures/devices.json";
import errorInvalid from "./fixtures/error-invalid.json";
import errorNotFound from "./fixtures/error-not-found.json";
import errorRateLimited from "./fixtures/error-rate-limited.json";
import me from "./fixtures/me.json";
import members from "./fixtures/members.json";
import pendingDevice from "./fixtures/pending-device.json";
import rustRoutes from "./fixtures/routes.json";
import schema from "./fixtures/schema.json";
import survey from "./fixtures/survey.json";
import surveys from "./fixtures/surveys.json";
import tower from "./fixtures/tower.json";
import towers from "./fixtures/towers.json";

const cases: [string, z.ZodType, unknown][] = [
  ["me", S.Me, me],
  ["surveys", S.SurveyList, surveys],
  ["survey", S.SurveyDetail, survey],
  ["towers", S.TowerCollection, towers],
  ["tower", S.TowerDetail, tower],
  ["pending device", S.PendingDevice, pendingDevice],
  ["devices", S.DeviceList, devices],
  ["members", S.MemberList, members],
  ["schema", S.DomainSchema, schema],
  ["invalid field error", S.ApiErrorBody, errorInvalid],
  ["rate limit error", S.ApiErrorBody, errorRateLimited],
  ["not found error", S.ApiErrorBody, errorNotFound],
];

describe("schemas read what the server writes", () => {
  it.each(cases)("%s", (_, schema, json) => {
    const r = schema.safeParse(json);
    expect(r.success, r.success ? "" : JSON.stringify(r.error.issues, null, 2)).toBe(true);
  });

  it("keeps markup shapes and colours", () => {
    const t = S.TowerDetail.parse(tower);
    const markup = t.photos[0]?.markup ?? [];
    expect(markup.map((m) => m.kind)).toEqual(["rect", "marker"]);
    expect(markup[0]?.color).toBe("#e51a1a");
  });

  it("reads towers longitude first, and keeps an undrawn tower in the list", () => {
    const fc = S.TowerCollection.parse(towers);
    expect(fc.features[0]?.geometry?.coordinates).toEqual([85.1, 25.6]);
    expect(fc.features.some((f) => f.geometry === null)).toBe(true);
  });

  it("refuses a body that isn't the expected shape", () => {
    expect(S.TowerCollection.safeParse({ type: "FeatureCollection", features: [{ type: "Feature" }] }).success).toBe(false);
    expect(S.SurveySummary.safeParse({ ...S.SurveyList.parse(surveys).surveys[0], severityCounts: [1, 2] }).success).toBe(false);
    expect(S.SurveyId.safeParse("srv_../../etc").success).toBe(false);
  });

  it("reads a plan it doesn't know as unknown, never as a paid plan", () => {
    expect(S.Plan.parse("enterprise")).toBe("unknown");
    expect(S.Plan.parse("pro")).toBe("pro");
  });
});

describe("routes", () => {
  it("match the server's", () => {
    const server = new Set((rustRoutes as [string, string][]).map(([m, p]) => `${m} ${p}`));
    for (const [name, [method, p]] of Object.entries(ROUTES)) {
      expect(server.has(`${method} ${p}`), name).toBe(true);
    }
  });

  it("encode parameters so they can't change the route", () => {
    expect(path("SURVEY_ASSET", { id: "srv_1", asset: "T/12 ?#" })).toBe("/api/v1/surveys/srv_1/assets/T%2F12%20%3F%23");
    expect(path("SURVEY_ASSET", { id: "srv_1", asset: ".." })).toBe("/api/v1/surveys/srv_1/assets/%2E%2E");
    expect(path("SURVEY_ASSET", { id: "srv_1", asset: "." })).toBe("/api/v1/surveys/srv_1/assets/%2E");
    expect(() => path("SURVEY", {})).toThrow();
  });
});

describe("ApiFailure", () => {
  it("knows when the person has to sign in again", () => {
    const body = S.ApiErrorBody.parse({ error: "unauthenticated", message: "Sign in." });
    expect(new ApiFailure("http", "Sign in.", 401, body).signedOut).toBe(true);
    expect(new ApiFailure("network", "offline").signedOut).toBe(false);
    expect(new ApiFailure("network", "offline").code).toBeNull();
  });
});
