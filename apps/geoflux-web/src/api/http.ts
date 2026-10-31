// The GeoFluxApi over fetch. Every body is parsed with ./schemas.ts before a page sees it, and
// every failure becomes an ApiFailure, so pages handle one error type.
//
// The app is served by the same server it calls, so requests are same-origin: the session cookie
// goes with them and nothing here ever reads or stores it.

import type { z } from "zod";

import { ApiFailure, type GeoFluxApi, type SignUpRequest } from "./client";
import { path, ROUTES, type RouteName } from "./routes";
import * as S from "./schemas";

/** Header every request other than GET carries; see client.ts. */
export const CSRF_HEADER = "X-GeoFlux-CSRF";

interface Call {
  params?: Record<string, string>;
  query?: Record<string, string>;
  body?: unknown;
  signal?: AbortSignal | undefined;
}

type Fetch = (input: string, init: RequestInit) => Promise<Response>;

export class HttpApi implements GeoFluxApi {
  /**
   * `base` is the server's origin with no trailing slash; empty (the default) means the page's
   * own. `fetchImpl` is for tests.
   */
  constructor(
    private readonly base = "",
    private readonly fetchImpl: Fetch = (input, init) => globalThis.fetch(input, init),
  ) {}

  private url(route: RouteName, params?: Record<string, string>, query?: Record<string, string>): string {
    const q = query && Object.keys(query).length > 0 ? `?${new URLSearchParams(query).toString()}` : "";
    return `${this.base}${path(route, params)}${q}`;
  }

  /** Sends the request; resolves only for a 2xx answer, otherwise rejects with an ApiFailure. */
  private async send(route: RouteName, call: Call = {}): Promise<Response> {
    const method = ROUTES[route][0];
    const headers: Record<string, string> = { Accept: "application/json" };
    if (method !== "GET") headers[CSRF_HEADER] = "1";
    if (call.body !== undefined) headers["Content-Type"] = "application/json";
    let res: Response;
    try {
      res = await this.fetchImpl(this.url(route, call.params, call.query), {
        method,
        headers,
        credentials: "same-origin",
        ...(call.body !== undefined ? { body: JSON.stringify(call.body) } : {}),
        ...(call.signal ? { signal: call.signal } : {}),
      });
    } catch (e) {
      if (call.signal?.aborted) throw new ApiFailure("aborted", "Cancelled.");
      throw new ApiFailure("network", `Can't reach GeoFlux: ${e instanceof Error ? e.message : String(e)}`);
    }
    if (res.ok) return res;
    const text = await readText(res, call.signal);
    const body = S.ApiErrorBody.safeParse(parseJson(text));
    if (body.success) throw new ApiFailure("http", body.data.message, res.status, body.data);
    // Not the server's own error: a proxy or load balancer answered. A gateway error means the
    // server isn't there, which the app treats like being offline.
    if (res.status === 502 || res.status === 503 || res.status === 504) {
      throw new ApiFailure("network", `GeoFlux is not answering (HTTP ${res.status}).`, res.status);
    }
    throw new ApiFailure("invalid_response", `Unexpected answer from the server (HTTP ${res.status}).`, res.status);
  }

  private async json<T>(schema: z.ZodType<T>, route: RouteName, call: Call = {}): Promise<T> {
    const res = await this.send(route, call);
    return parseBody(schema, await readText(res, call.signal), route);
  }

  /** A read where "not found" is an answer, not a failure. */
  private async maybe<T>(schema: z.ZodType<T>, route: RouteName, call: Call = {}): Promise<T | null> {
    try {
      return await this.json(schema, route, call);
    } catch (e) {
      if (e instanceof ApiFailure && e.code === "not_found") return null;
      throw e;
    }
  }

  private async none(route: RouteName, call: Call = {}): Promise<void> {
    const res = await this.send(route, call);
    // Read the (empty) body so the connection can be reused.
    await readText(res, call.signal);
  }

  async me(signal?: AbortSignal) {
    try {
      return await this.json(S.Me, "ME", { signal });
    } catch (e) {
      if (e instanceof ApiFailure && e.signedOut) return null;
      throw e;
    }
  }
  signUp(request: SignUpRequest) {
    return this.json(S.Me, "SIGN_UP", { body: request });
  }
  logIn(email: string, password: string) {
    return this.json(S.Me, "LOG_IN", { body: { email, password } });
  }
  logOut() {
    return this.none("LOG_OUT");
  }
  forgotPassword(email: string) {
    return this.none("FORGOT_PASSWORD", { body: { email } });
  }
  resetPassword(token: string, password: string) {
    return this.none("RESET_PASSWORD", { body: { token, password } });
  }
  verifyEmail(token: string) {
    return this.none("VERIFY_EMAIL", { body: { token } });
  }
  acceptInvite(token: string, name: string, password: string) {
    return this.none("ACCEPT_INVITE", { body: { token, name, password } });
  }

  pendingDevice(userCode: string, signal?: AbortSignal) {
    return this.maybe(S.PendingDevice, "DEVICE_PENDING", { query: { code: userCode }, signal });
  }
  answerDevice(userCode: string, approve: boolean) {
    return this.none("DEVICE_APPROVE", { body: { userCode, approve } });
  }

  async surveys(signal?: AbortSignal) {
    return (await this.json(S.SurveyList, "SURVEYS", { signal })).surveys;
  }
  survey(id: string, signal?: AbortSignal) {
    return this.maybe(S.SurveyDetail, "SURVEY", { params: { id }, signal });
  }
  towers(id: string, signal?: AbortSignal) {
    return this.maybe(S.TowerCollection, "SURVEY_TOWERS", { params: { id }, signal });
  }
  tower(id: string, assetId: string, signal?: AbortSignal) {
    return this.maybe(S.TowerDetail, "SURVEY_ASSET", { params: { id, asset: assetId }, signal });
  }
  schema(id: string, signal?: AbortSignal) {
    return this.maybe(S.DomainSchema, "SCHEMA", { params: { id }, signal });
  }
  lineHealth(id: string, query: S.HealthQuery, signal?: AbortSignal) {
    // Only the filters that are set: the server refuses an empty one.
    const q: Record<string, string> = {};
    for (const [k, v] of Object.entries(query)) if (typeof v === "string" && v !== "") q[k] = v;
    return this.maybe(S.LineHealth, "SURVEY_HEALTH", { params: { id }, query: q, signal });
  }
  towerHealth(id: string, assetId: string, signal?: AbortSignal) {
    return this.maybe(S.TowerHealth, "TOWER_HEALTH", { params: { id, asset: assetId }, signal });
  }

  members(signal?: AbortSignal) {
    return this.json(S.MemberList, "MEMBERS", { signal });
  }
  invite(email: string, role: S.Role) {
    return this.none("INVITE", { body: { email, role } });
  }
  setRole(userId: string, role: S.Role) {
    return this.none("SET_ROLE", { params: { id: userId }, body: { role } });
  }
  removeMember(userId: string) {
    return this.none("REMOVE_MEMBER", { params: { id: userId } });
  }
  devices(signal?: AbortSignal) {
    return this.json(S.DeviceList, "DEVICES", { signal });
  }
  revokeDevice(deviceId: string) {
    return this.none("REVOKE_DEVICE", { params: { id: deviceId } });
  }

  photoUrl(surveyId: string, sha256: string) {
    return this.url("PHOTO", { id: surveyId, sha256 });
  }
  towerReportUrl(surveyId: string, assetId: string) {
    return this.url("TOWER_REPORT", { id: surveyId, asset: assetId });
  }
  lineReportUrl(surveyId: string) {
    return this.url("LINE_REPORT", { id: surveyId });
  }
}

async function readText(res: Response, signal: AbortSignal | undefined): Promise<string> {
  try {
    return await res.text();
  } catch (e) {
    if (signal?.aborted) throw new ApiFailure("aborted", "Cancelled.");
    throw new ApiFailure("network", `The answer was cut off: ${e instanceof Error ? e.message : String(e)}`, res.status);
  }
}

function parseJson(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

function parseBody<T>(schema: z.ZodType<T>, text: string, route: RouteName): T {
  const parsed = schema.safeParse(parseJson(text));
  if (parsed.success) return parsed.data;
  const where = parsed.error.issues[0]?.path.join(".") ?? "";
  throw new ApiFailure("invalid_response", `Unexpected answer to ${route}${where ? ` at ${where}` : ""}.`);
}

/** What to tell the person about a failure, in a sentence. */
export function describeFailure(e: unknown): string {
  if (!(e instanceof ApiFailure)) return "Something went wrong. Try again.";
  switch (e.kind) {
    case "network":
      return "Can't reach GeoFlux. Check your connection and try again.";
    case "invalid_response":
      return "GeoFlux sent an answer this page doesn't understand. Reload the page and try again.";
    case "aborted":
      return "Cancelled.";
    case "http":
      if (e.code === "rate_limited") {
        const secs = e.body?.retryAfter ?? 60;
        const mins = Math.max(1, Math.ceil(secs / 60));
        return `Too many tries. Wait ${mins} minute${mins === 1 ? "" : "s"} and try again.`;
      }
      return e.message || "Something went wrong. Try again.";
  }
}

/** The server's reason for one form field, when it named one. */
export function fieldReason(e: unknown, field: string): string | null {
  if (!(e instanceof ApiFailure)) return null;
  return e.body?.fields?.find((f) => f.field === field)?.reason ?? null;
}

/** Whether the server pointed at form fields, which the form shows next to each one. */
export function hasFieldErrors(e: unknown): boolean {
  return e instanceof ApiFailure && (e.body?.fields?.length ?? 0) > 0;
}
