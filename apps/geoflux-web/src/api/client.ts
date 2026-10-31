// The web app's view of the server: one interface the pages call, so a page never builds a URL or
// reads a raw response, and tests can hand pages a fake. The fetch-based implementation comes in
// stage 3 (http.ts); it parses every body with ./schemas.ts.
//
// Signing in sets an HttpOnly, SameSite=Strict session cookie the page can't read. Every request
// other than GET also sends `X-GeoFlux-CSRF: 1`, which a form on another site can't add.

import type {
  DeviceList,
  DomainSchema,
  HealthQuery,
  LineHealth,
  Me,
  MemberList,
  PendingDevice,
  Role,
  SurveyDetail,
  SurveySummary,
  TowerCollection,
  TowerDetail,
  TowerHealth,
  ApiErrorBody,
  ErrorCode,
} from "./schemas";

/** Why a call failed. Pages branch on `kind` and, for `http`, on `body.error`. */
export type FailureKind =
  /** The server answered with an error body (4xx or 5xx). */
  | "http"
  /** No answer: offline, DNS, the server down. The PWA shows what it has cached. */
  | "network"
  /** An answer that isn't what this app expects (wrong JSON, a proxy's HTML page). */
  | "invalid_response"
  /** The page cancelled the call (it navigated away). Never shown to the person. */
  | "aborted";

export class ApiFailure extends Error {
  override readonly name = "ApiFailure";

  constructor(
    readonly kind: FailureKind,
    message: string,
    /** The HTTP status, when there was one. */
    readonly status: number | null = null,
    /** The server's error body, for `http`. */
    readonly body: ApiErrorBody | null = null,
  ) {
    super(message);
  }

  /** The server's error code, when it sent one. */
  get code(): ErrorCode | null {
    return this.body?.error ?? null;
  }

  /** The session ran out or was never there: the app goes to the log-in page. */
  get signedOut(): boolean {
    return this.code === "unauthenticated" || this.code === "revoked";
  }
}

export interface SignUpRequest {
  email: string;
  password: string;
  name: string;
  organisation: string;
}

/**
 * Every call the web app makes. All of them reject with an {@link ApiFailure}, never anything else.
 * A read that finds nothing (another organisation's survey, an uploading one, a tower id that
 * isn't there) resolves to `null`, so pages show "not found" without catching.
 */
export interface GeoFluxApi {
  // Signing in. `me` is `null` when nobody is signed in.
  me(signal?: AbortSignal): Promise<Me | null>;
  signUp(request: SignUpRequest): Promise<Me>;
  logIn(email: string, password: string): Promise<Me>;
  logOut(): Promise<void>;
  forgotPassword(email: string): Promise<void>;
  resetPassword(token: string, password: string): Promise<void>;
  verifyEmail(token: string): Promise<void>;
  acceptInvite(token: string, name: string, password: string): Promise<void>;

  // Approving GeoGIS's sign-in code on the /device page. `pendingDevice` is `null` for a code
  // that doesn't exist, ran out or was already answered.
  pendingDevice(userCode: string, signal?: AbortSignal): Promise<PendingDevice | null>;
  answerDevice(userCode: string, approve: boolean): Promise<void>;

  // Surveys, newest upload first. Only committed ones.
  surveys(signal?: AbortSignal): Promise<SurveySummary[]>;
  survey(id: string, signal?: AbortSignal): Promise<SurveyDetail | null>;
  towers(id: string, signal?: AbortSignal): Promise<TowerCollection | null>;
  tower(id: string, assetId: string, signal?: AbortSignal): Promise<TowerDetail | null>;
  schema(id: string, signal?: AbortSignal): Promise<DomainSchema | null>;

  // The line health dashboard. A filter the server refuses rejects with an `invalid_field` failure.
  lineHealth(id: string, query: HealthQuery, signal?: AbortSignal): Promise<LineHealth | null>;
  towerHealth(id: string, assetId: string, signal?: AbortSignal): Promise<TowerHealth | null>;

  // The organisation (admins).
  members(signal?: AbortSignal): Promise<MemberList>;
  invite(email: string, role: Role): Promise<void>;
  setRole(userId: string, role: Role): Promise<void>;
  removeMember(userId: string): Promise<void>;
  devices(signal?: AbortSignal): Promise<DeviceList>;
  revokeDevice(deviceId: string): Promise<void>;

  // Files the browser loads itself with the session cookie (an <img> or a download link), so a
  // large photo or ZIP is never held in the page's memory.
  photoUrl(surveyId: string, sha256: string): string;
  towerReportUrl(surveyId: string, assetId: string): string;
  lineReportUrl(surveyId: string): string;
}

/** The web pages the server's emails and GeoGIS link to. Their paths must not change. */
export const PAGES = {
  device: "/device", // ?code=WDJB-MJHT, from GeoGIS's Account window
  verifyEmail: "/verify-email", // ?token=
  resetPassword: "/reset-password", // ?token=
  invite: "/invite", // ?token=
} as const;
