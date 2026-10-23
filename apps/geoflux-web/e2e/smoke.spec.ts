// One pass through the app as people use it, against the real server: an inspector's account and
// a survey synced from GeoGIS (made through the API, as GeoGIS would), then in the browser: log in,
// approve a GeoGIS sign-in, open the survey, a tower, its photo with markup and its PDF, use it
// offline, log out.
//
// Email links aren't sent: the server writes them to its log, which GEOFLUX_E2E_LOG names.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

import { type APIRequestContext, expect, test } from "@playwright/test";

const EMAIL = `inspector-${Date.now()}@example.com`;
const PASSWORD = "correct horse battery";
const CSRF = { "X-GeoFlux-CSRF": "1" };

// A 64×48 JPEG; every photo in the survey is this one.
const PHOTO = Buffer.from(
  "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDABALDA4MChAODQ4SERATGCgaGBYWGDEjJR0oOjM9PDkzODdASFxOQERXRTc4UG1RV19iZ2hnPk1xeXBkeFxlZ2P/2wBDARESEhgVGC8aGi9jQjhCY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2NjY2P/wAARCAAwAEADASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwDmaKKKg9cKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigAooooAKKKKACiiigD//2Q==",
  "base64",
);


/** The last link the server logged for `email`, waiting a moment for the log to be written. */
async function loggedLink(email: string): Promise<string> {
  const log = process.env["GEOFLUX_E2E_LOG"];
  if (!log) throw new Error("Set GEOFLUX_E2E_LOG to the server's log file.");
  for (let i = 0; i < 20; i++) {
    const lines = readFileSync(log, "utf8").split("\n").filter((l) => l.includes(`email for ${email} `));
    const link = lines.at(-1)?.match(/(https?:\/\/\S+)/)?.[1];
    if (link) return link;
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`No email link for ${email} in ${log}`);
}

async function ok<T>(res: Promise<import("@playwright/test").APIResponse>): Promise<T> {
  const r = await res;
  if (!r.ok()) throw new Error(`${r.url()}: ${r.status()} ${await r.text()}`);
  const text = await r.text();
  return (text ? JSON.parse(text) : null) as T;
}

/** GeoGIS's sign-in on a computer: a code, approved by `approve`, then the computer's token. */
async function signInGeoGis(api: APIRequestContext, approve: (userCode: string) => Promise<void>): Promise<string> {
  const code = await ok<{ deviceCode: string; userCode: string }>(
    api.post("/api/v1/device/code", { data: { machine: `e2e-${Date.now()}`, deviceName: "SURVEY-LAPTOP" } }),
  );
  await approve(code.userCode);
  const token = await ok<{ accessToken: string }>(api.post("/api/v1/device/token", { data: { deviceCode: code.deviceCode } }));
  return token.accessToken;
}

test("an inspector's day on the web app", async ({ page, context, request }) => {
  const problems: string[] = [];
  page.on("pageerror", (e) => problems.push(String(e)));
  page.on("console", (m) => {
    if (/Content Security Policy/i.test(m.text())) problems.push(m.text());
  });

  // An account with a confirmed email, made the way people make one.
  await ok(request.post("/api/v1/auth/signup", { headers: CSRF, data: { email: EMAIL, password: PASSWORD, name: "Asha Inspector", organisation: "Bihar Grid" } }));
  const verify = new URL(await loggedLink(EMAIL));
  await page.goto(verify.pathname + verify.search);
  await expect(page.getByText(/email address is confirmed/)).toBeVisible();

  // GeoGIS signs in on a laptop; the inspector approves it in the browser.
  await page.goto("/login");
  await page.getByLabel("Email").fill(EMAIL);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: "Log in" }).click();
  await expect(page.getByRole("heading", { name: "Surveys" })).toBeVisible();
  const token = await signInGeoGis(request, async (userCode) => {
    await page.goto(`/device?code=${encodeURIComponent(userCode)}`);
    await expect(page.getByText("SURVEY-LAPTOP")).toBeVisible();
    await page.getByRole("button", { name: "Approve" }).click();
    await expect(page.getByText(/Go back to GeoGIS/)).toBeVisible();
  });

  // GeoGIS syncs a three-tower survey; tower 102 has a finding with markup.
  const photo = createHash("sha256").update(PHOTO).digest("hex");
  const markup = [{ kind: "rect", a: { x: 0.3, y: 0.3 }, b: { x: 0.6, y: 0.7 }, color: "#e51a1a", width: 6 }];
  const tower = (n: number, severity: string) => ({
    asset: {
      id: `t${n}`,
      schemaId: "com.geo.telecom",
      position: n,
      status: "ready",
      location: { lat: 25.6 + n * 0.004, lon: 85.1 + n * 0.006 },
      properties: [["towerNumber", `${101 + n}`], ["lineName", "Line 4"]],
      severity,
    },
    photos: [
      { slot: "THERMAL", sha256: photo, bytes: PHOTO.length, type: "jpeg", fileName: `DJI_${n}_T.JPG`, ...(severity === "none" ? {} : { markup }) },
    ],
  });
  const bearer = { Authorization: `Bearer ${token}` };
  const survey = await ok<{ survey: string }>(
    request.post("/api/v1/surveys", {
      headers: bearer,
      data: {
        format: 1,
        clientSurveyId: `e2e-${Date.now()}`,
        revision: 1,
        project: "Patna 132 kV",
        line: "Line 4",
        schemaId: "com.geo.telecom",
        assets: [tower(0, "none"), tower(1, "high"), tower(2, "none")],
      },
    }),
  );
  await ok(request.put(`/api/v1/surveys/${survey.survey}/photos/${photo}`, { headers: { ...bearer, "Content-Type": "image/jpeg" }, data: PHOTO }));
  await ok(request.post(`/api/v1/surveys/${survey.survey}/commit`, { headers: bearer }));

  // The survey, its map and a tower with its photo, markup and PDF.
  await page.getByRole("link", { name: "Surveys" }).click();
  await page.getByRole("link", { name: "Line 4" }).click();
  await expect(page.getByRole("region", { name: "Map of the line's towers" }).locator("canvas")).toBeVisible();
  await page.getByRole("navigation", { name: "Towers" }).getByRole("button", { name: /^102/ }).click();
  const panel = page.getByRole("region", { name: "Tower 102" });
  const img = panel.getByRole("img", { name: /tower 102/ });
  await expect(img).toBeVisible();
  await expect.poll(() => img.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth)).toBe(64);
  await expect(panel.locator("svg rect")).toHaveCount(1);
  const pdfHref = await panel.getByRole("link", { name: "Tower PDF", exact: true }).getAttribute("href");
  const pdf = await page.request.get(pdfHref ?? "");
  expect(pdf.headers()["content-type"]).toBe("application/pdf");
  expect((await pdf.body()).subarray(0, 5).toString()).toBe("%PDF-");

  // Offline: once the app has been opened, the survey and its photos are still there.
  await expect.poll(() => page.evaluate(() => !!navigator.serviceWorker?.controller), { timeout: 15_000 }).toBe(true);
  await page.reload();
  await expect(img).toBeVisible();
  const here = page.url();
  await context.setOffline(true);
  await page.goto(here);
  await expect(page.getByText("You're offline")).toBeVisible();
  await expect(page.getByRole("region", { name: "Tower 102" }).getByRole("img", { name: /tower 102/ })).toBeVisible();
  await context.setOffline(false);

  // Logging out forgets the account's data on this browser.
  await page.getByRole("button", { name: "Log out" }).click();
  await page.waitForURL(/\/login/);
  expect(await page.evaluate(() => caches.keys())).toEqual(["geoflux-app-v1"]);

  expect(problems).toEqual([]);
});
