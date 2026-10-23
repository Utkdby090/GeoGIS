// The team and computers pages.

import { cleanup, fireEvent, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiFailure } from "../api/client";
import * as S from "../api/schemas";
import { fakeApi, ME } from "../test/fake-api";
import { renderApp } from "../test/render";
import { seatsInUse } from "./team";

import devicesFixture from "../api/fixtures/devices.json";
import membersFixture from "../api/fixtures/members.json";

afterEach(cleanup);

const MEMBERS = S.MemberList.parse(membersFixture);
const BOB: S.MemberList["members"][number] = {
  user: { id: "usr_09090909090909090909090909090909", email: "bob@example.com", name: "Bob", emailVerified: true, created: 1 },
  role: "inspector",
  joined: 1_790_000_000,
};
const TEAM: S.MemberList = { ...MEMBERS, members: [...MEMBERS.members, BOB] };
const DEVICES = S.DeviceList.parse(devicesFixture);
const signedIn = (me: S.Me, more: Parameters<typeof fakeApi>[0] = {}) => fakeApi({ me: vi.fn(() => Promise.resolve(me)), ...more });
const asRole = (role: S.Role): S.Me => ({ ...ME, membership: { ...ME.membership, role } });

describe("the team page", () => {
  it("lists people and seats, and changes a role", async () => {
    const api = signedIn(ME, { members: vi.fn(() => Promise.resolve(TEAM)), setRole: vi.fn(() => Promise.resolve()) });
    renderApp(api, "/team");
    expect(await screen.findByText("Inspectors and admins: 2 of 3")).toBeTruthy();
    expect((screen.getByRole("combobox", { name: `Role of ${ME.user.name}` }) as HTMLSelectElement).disabled).toBe(true);
    fireEvent.change(screen.getByRole("combobox", { name: "Role of Bob" }), { target: { value: "admin" } });
    await waitFor(() => expect(api.setRole).toHaveBeenCalledWith(BOB.user.id, "admin"));
    // The list is read again after a change.
    await waitFor(() => expect(api.members).toHaveBeenCalledTimes(2));
  });

  it("asks before removing someone, and never offers to remove yourself", async () => {
    const api = signedIn(ME, { members: vi.fn(() => Promise.resolve(TEAM)), removeMember: vi.fn(() => Promise.resolve()) });
    renderApp(api, "/team");
    await screen.findByText("bob@example.com");
    expect(screen.getAllByRole("button", { name: "Remove" })).toHaveLength(1);
    const confirm = vi.spyOn(window, "confirm").mockReturnValueOnce(false).mockReturnValueOnce(true);
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(api.removeMember).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(api.removeMember).toHaveBeenCalledWith(BOB.user.id));
    expect(confirm).toHaveBeenCalledTimes(2);
  });

  it("invites, and offers viewers only on the Organisation plan", async () => {
    const api = signedIn(ME, { members: vi.fn(() => Promise.resolve(TEAM)), invite: vi.fn(() => Promise.resolve()) });
    renderApp(api, "/team");
    const card = (await screen.findByRole("heading", { name: "Invite someone" })).parentElement!;
    expect((within(card).getByRole("option", { name: /^Viewer/ }) as HTMLOptionElement).disabled).toBe(true);
    fireEvent.change(within(card).getByLabelText("Email"), { target: { value: "new@example.com" } });
    fireEvent.click(within(card).getByRole("button", { name: "Send invitation" }));
    await within(card).findByText("new@example.com");
    expect(api.invite).toHaveBeenCalledWith("new@example.com", "inspector");
  });

  it("warns when every seat is taken", async () => {
    const api = signedIn(ME, { members: vi.fn(() => Promise.resolve({ ...TEAM, seats: 2 })) });
    renderApp(api, "/team");
    expect(await screen.findByText(/seat on your plan is taken/)).toBeTruthy();
  });

  it("says when someone already has an account", async () => {
    const api = signedIn(ME, {
      members: vi.fn(() => Promise.resolve(TEAM)),
      invite: vi.fn(() => Promise.reject(new ApiFailure("http", "x", 409, { error: "conflict", message: "x" }))),
    });
    renderApp(api, "/team");
    fireEvent.click(await screen.findByRole("button", { name: "Send invitation" }));
    expect((await screen.findByRole("alert")).textContent).toBe("This person already has a GeoFlux account.");
  });

  it("is for admins only, and only admins see it in the menu", async () => {
    const api = signedIn(asRole("inspector"));
    renderApp(api, "/team");
    await screen.findByText(/Only admins of/);
    expect(screen.queryByRole("link", { name: "Team" })).toBeNull();
    expect(api.members).not.toHaveBeenCalled();
  });
});

describe("the computers page", () => {
  it("shows the person on each computer to an admin, and signs one out", async () => {
    const theirs = { ...DEVICES.devices[0]!, id: "dev_06060606060606060606060606060606", user: BOB.user.id, name: "FIELD-PC" };
    const api = signedIn(ME, {
      devices: vi.fn(() => Promise.resolve({ devices: [...DEVICES.devices, theirs] })),
      members: vi.fn(() => Promise.resolve(TEAM)),
      revokeDevice: vi.fn(() => Promise.resolve()),
    });
    renderApp(api, "/computers");
    const row = (await screen.findByText("FIELD-PC")).closest("tr")!;
    await within(row).findByText("Bob");
    expect(screen.getByText("You")).toBeTruthy();
    vi.spyOn(window, "confirm").mockReturnValue(true);
    fireEvent.click(within(row).getByRole("button", { name: "Sign out" }));
    await waitFor(() => expect(api.revokeDevice).toHaveBeenCalledWith(theirs.id));
  });

  it("shows an inspector only their own, without a person column", async () => {
    const api = signedIn(asRole("inspector"), { devices: vi.fn(() => Promise.resolve(DEVICES)) });
    renderApp(api, "/computers");
    await screen.findByText(DEVICES.devices[0]!.name);
    expect(screen.queryByRole("columnheader", { name: "Person" })).toBeNull();
    expect(api.members).not.toHaveBeenCalled();
  });
});

it("counts viewers apart from paid seats", () => {
  const viewer = { ...BOB, role: "viewer" as const };
  expect(seatsInUse({ ...TEAM, members: [...TEAM.members, viewer] })).toEqual({ full: 2, viewers: 1 });
});
