// The sign-in pages and the GeoGIS approve page, driven as a person would.

import { cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiFailure } from "../api/client";
import { planText } from "../app/layout";
import { safeNext } from "../app/session";
import { fakeApi, ME } from "../test/fake-api";
import { renderApp } from "../test/render";
import { tidyCode } from "./device";

afterEach(cleanup);

const here = () => screen.getByTestId("here").textContent;
const http = (code: "invalid_credentials" | "forbidden" | "not_found", message: string, status = 400) =>
  new ApiFailure("http", message, status, { error: code, message });

function type(label: string, value: string) {
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
}

describe("signing in", () => {
  it("sends someone not signed in to log in, then back where they were going", async () => {
    let signedIn = false;
    const api = fakeApi({
      me: vi.fn(() => Promise.resolve(signedIn ? ME : null)),
      logIn: vi.fn(() => {
        signedIn = true;
        return Promise.resolve(ME);
      }),
      pendingDevice: vi.fn(() => Promise.resolve({ deviceName: "SURVEY-LAPTOP", expires: 4_102_444_800 })),
    });
    renderApp(api, "/device?code=wdjb%20mjht");
    await waitFor(() => expect(here()).toBe(`/login?next=${encodeURIComponent("/device?code=wdjb%20mjht")}`));
    type("Email", "inspector@example.com");
    type("Password", "correct horse battery");
    fireEvent.click(screen.getByRole("button", { name: "Log in" }));
    await screen.findByText("SURVEY-LAPTOP");
    expect(api.logIn).toHaveBeenCalledWith("inspector@example.com", "correct horse battery");
    expect(api.pendingDevice).toHaveBeenCalledWith("WDJB-MJHT", expect.anything());
  });

  it("shows the server's reason when logging in fails", async () => {
    const api = fakeApi({ logIn: vi.fn(() => Promise.reject(http("invalid_credentials", "Wrong email or password.", 401))) });
    renderApp(api, "/login");
    fireEvent.click(await screen.findByRole("button", { name: "Log in" }));
    expect(await screen.findByRole("alert")).toHaveProperty("textContent", "Wrong email or password.");
  });

  it("puts a field's problem next to that field on sign-up", async () => {
    const api = fakeApi({
      signUp: vi.fn(() =>
        Promise.reject(
          new ApiFailure("http", "Check the form.", 400, {
            error: "invalid_field",
            message: "Check the form.",
            fields: [{ field: "password", reason: "too_short" }],
          }),
        ),
      ),
    });
    renderApp(api, "/signup");
    fireEvent.click(await screen.findByRole("button", { name: "Create account" }));
    expect(await screen.findByText("Use at least 10 characters.")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("never sends anyone off the site after logging in", () => {
    expect(safeNext("/device?code=X")).toBe("/device?code=X");
    for (const bad of ["//evil.example", "/\\evil.example", "https://evil.example", "/\t/evil.example", "", null]) {
      expect(safeNext(bad)).toBe("/");
    }
  });
});

describe("approving GeoGIS's code", () => {
  const signedIn = (more: Parameters<typeof fakeApi>[0] = {}) =>
    fakeApi({ me: vi.fn(() => Promise.resolve(ME)), ...more });

  it("names the computer, then approves", async () => {
    const api = signedIn({
      pendingDevice: vi.fn(() => Promise.resolve({ deviceName: "<b>FIELD-PC</b>", expires: 4_102_444_800 })),
      answerDevice: vi.fn(() => Promise.resolve()),
    });
    renderApp(api, "/device?code=WDJB-MJHT");
    // Shown as text: a computer name is never read as HTML.
    expect(await screen.findByText("<b>FIELD-PC</b>")).toBeTruthy();
    expect(screen.getAllByText(ME.organisation.name)).toHaveLength(2); // header and question
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await screen.findByText(/Go back to GeoGIS/);
    expect(api.answerDevice).toHaveBeenCalledWith("WDJB-MJHT", true);
  });

  it("can deny", async () => {
    const api = signedIn({
      pendingDevice: vi.fn(() => Promise.resolve({ deviceName: "PC", expires: 4_102_444_800 })),
      answerDevice: vi.fn(() => Promise.resolve()),
    });
    renderApp(api, "/device?code=WDJB-MJHT");
    fireEvent.click(await screen.findByRole("button", { name: "Deny" }));
    await screen.findByText(/won't be signed in/);
    expect(api.answerDevice).toHaveBeenCalledWith("WDJB-MJHT", false);
  });

  it("says when a code is unknown or used up", async () => {
    const api = signedIn({ pendingDevice: vi.fn(() => Promise.resolve(null)) });
    renderApp(api, "/device?code=WDJB-MJHT");
    expect((await screen.findByRole("alert")).textContent).toMatch(/not valid or has run out/);
  });

  it("asks for the code when the link had none", async () => {
    const api = signedIn({ pendingDevice: vi.fn(() => Promise.resolve(null)) });
    renderApp(api, "/device");
    await screen.findByLabelText("Code");
    type("Code", " wdjb mjht ");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    await waitFor(() => expect(here()).toBe("/device?code=WDJB-MJHT"));
  });

  it("explains why a viewer can't approve", async () => {
    const api = signedIn({
      pendingDevice: vi.fn(() => Promise.resolve({ deviceName: "PC", expires: 4_102_444_800 })),
      answerDevice: vi.fn(() => Promise.reject(http("forbidden", "Not allowed.", 403))),
    });
    renderApp(api, "/device?code=WDJB-MJHT");
    fireEvent.click(await screen.findByRole("button", { name: "Approve" }));
    expect((await screen.findByRole("alert")).textContent).toMatch(/Viewers can't sign GeoGIS in/);
  });

  it("tidies a typed code", () => {
    expect(tidyCode(" wdjb mjht ")).toBe("WDJB-MJHT");
    expect(tidyCode("WDJB-MJHT")).toBe("WDJB-MJHT");
    expect(tidyCode("abc")).toBe("ABC");
  });
});

describe("the header", () => {
  it("shows the trial's days left", () => {
    const now = 1_000_000;
    const trial = { ...ME, subscription: { ...ME.subscription, state: { state: "trial" as const, ends: now + 86_400 * 3 - 10 } } };
    expect(planText(trial, now)).toMatch(/trial, 3 days left$/);
  });

  it("logs out and forgets the account", async () => {
    let signedIn = true;
    const api = fakeApi({
      me: vi.fn(() => Promise.resolve(signedIn ? ME : null)),
      logOut: vi.fn(() => {
        signedIn = false;
        return Promise.resolve();
      }),
    });
    renderApp(api, "/");
    fireEvent.click(await screen.findByRole("button", { name: "Log out" }));
    await waitFor(() => expect(here()).toBe("/login"));
    expect(api.logOut).toHaveBeenCalled();
  });
});
