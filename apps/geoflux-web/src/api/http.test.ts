// HttpApi against a fake fetch: what it sends, and how each kind of answer comes back.

import { describe, expect, it, vi } from "vitest";

import { ApiFailure } from "./client";
import { CSRF_HEADER, describeFailure, HttpApi } from "./http";

import errorNotFound from "./fixtures/error-not-found.json";
import errorRateLimited from "./fixtures/error-rate-limited.json";
import me from "./fixtures/me.json";
import pendingDevice from "./fixtures/pending-device.json";

type Sent = { url: string; init: RequestInit };

function fake(...answers: (Response | Error)[]) {
  const sent: Sent[] = [];
  const fetchImpl = vi.fn(async (url: string, init: RequestInit) => {
    sent.push({ url, init });
    const a = answers.shift();
    if (!a) throw new Error("no more answers");
    if (a instanceof Error) throw a;
    return a;
  });
  return { api: new HttpApi("", fetchImpl), sent };
}

const json = (status: number, body: unknown) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
const headers = (s: Sent) => s.init.headers as Record<string, string>;

async function failure(p: Promise<unknown>): Promise<ApiFailure> {
  try {
    await p;
  } catch (e) {
    expect(e).toBeInstanceOf(ApiFailure);
    return e as ApiFailure;
  }
  throw new Error("expected a failure");
}

describe("HttpApi", () => {
  it("sends the CSRF header on everything but GET", async () => {
    const { api, sent } = fake(json(200, me), json(200, me));
    await api.me();
    await api.logIn("a@b.co", "secret password");
    expect(headers(sent[0]!)[CSRF_HEADER]).toBeUndefined();
    expect(sent[1]!.init.method).toBe("POST");
    expect(headers(sent[1]!)[CSRF_HEADER]).toBe("1");
    expect(JSON.parse(sent[1]!.init.body as string)).toEqual({ email: "a@b.co", password: "secret password" });
    expect(sent[1]!.init.credentials).toBe("same-origin");
  });

  it("reads 'nobody signed in' as null, not a failure", async () => {
    const { api } = fake(json(401, { error: "unauthenticated", message: "Log in first." }));
    expect(await api.me()).toBeNull();
  });

  it("reads not found as null for reads", async () => {
    const { api, sent } = fake(json(404, errorNotFound));
    expect(await api.pendingDevice("WDJB MJHT&x=1")).toBeNull();
    expect(sent[0]!.url).toBe("/api/v1/device/pending?code=WDJB+MJHT%26x%3D1");
  });

  it("parses what it reads", async () => {
    const { api } = fake(json(200, pendingDevice));
    expect(await api.pendingDevice("WDJB-MJHT")).toEqual(pendingDevice);
  });

  it("turns the server's error into an http failure with its code", async () => {
    const { api } = fake(json(429, errorRateLimited));
    const e = await failure(api.forgotPassword("a@b.co"));
    expect(e.kind).toBe("http");
    expect(e.code).toBe("rate_limited");
    expect(describeFailure(e)).toMatch(/^Too many tries\. Wait \d+ minutes?/);
  });

  it("treats a proxy's gateway page as being offline", async () => {
    const { api } = fake(new Response("<html>Bad gateway</html>", { status: 502 }));
    expect((await failure(api.surveys())).kind).toBe("network");
  });

  it("rejects a body that isn't what the app expects", async () => {
    const { api } = fake(json(200, { surveys: [{ id: "nope" }] }), new Response("<html>", { status: 200 }));
    expect((await failure(api.surveys())).kind).toBe("invalid_response");
    expect((await failure(api.surveys())).kind).toBe("invalid_response"); // an HTML page
  });

  it("reports a dropped connection and a cancelled call apart", async () => {
    const { api } = fake(new TypeError("Failed to fetch"));
    expect((await failure(api.surveys())).kind).toBe("network");
    const ctl = new AbortController();
    ctl.abort();
    const aborted = new HttpApi("", async () => {
      throw new DOMException("aborted", "AbortError");
    });
    expect((await failure(aborted.surveys(ctl.signal))).kind).toBe("aborted");
  });

  it("builds file URLs with each part encoded", () => {
    const api = new HttpApi("https://geoflux.example");
    expect(api.towerReportUrl("srv_1", "T/1 ..")).toBe("https://geoflux.example/api/v1/surveys/srv_1/assets/T%2F1%20../report.pdf");
    expect(api.towerReportUrl("srv_1", "..")).toBe("https://geoflux.example/api/v1/surveys/srv_1/assets/%2E%2E/report.pdf");
  });
});
