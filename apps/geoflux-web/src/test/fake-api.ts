// A GeoFluxApi for page tests: every call rejects unless the test gives it an answer.

import { vi } from "vitest";

import type { GeoFluxApi } from "../api/client";
import * as S from "../api/schemas";
import meFixture from "../api/fixtures/me.json";

export const ME: S.Me = S.Me.parse(meFixture);

export function fakeApi(overrides: Partial<GeoFluxApi> = {}): GeoFluxApi {
  const no = (name: string) => vi.fn(() => Promise.reject(new Error(`unexpected call: ${name}`)));
  const api: GeoFluxApi = {
    me: vi.fn(() => Promise.resolve(null)),
    signUp: no("signUp"),
    logIn: no("logIn"),
    logOut: vi.fn(() => Promise.resolve()),
    forgotPassword: no("forgotPassword"),
    resetPassword: no("resetPassword"),
    verifyEmail: no("verifyEmail"),
    acceptInvite: no("acceptInvite"),
    pendingDevice: no("pendingDevice"),
    answerDevice: no("answerDevice"),
    surveys: vi.fn(() => Promise.resolve([])),
    survey: no("survey"),
    towers: no("towers"),
    tower: no("tower"),
    schema: no("schema"),
    members: no("members"),
    invite: no("invite"),
    setRole: no("setRole"),
    removeMember: no("removeMember"),
    devices: no("devices"),
    revokeDevice: no("revokeDevice"),
    photoUrl: (s, h) => `/photo/${s}/${h}`,
    towerReportUrl: (s, a) => `/report/${s}/${a}`,
    lineReportUrl: (s) => `/report/${s}`,
    ...overrides,
  };
  return api;
}
