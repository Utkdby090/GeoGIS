// /team: the organisation's people (admins only), and /computers: the computers signed in to
// GeoGIS (everyone sees their own; admins see everyone's).

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";

import { ApiFailure } from "../api/client";
import { describeFailure, fieldReason } from "../api/http";
import type { Me, MemberList, Role } from "../api/schemas";
import { useApi } from "../app/api-context";
import { useMe } from "../app/session";
import { Field, Loading, Notice, reasonText } from "../app/ui";

const ROLE_NAMES: Record<Role, string> = { admin: "Admin", inspector: "Inspector", viewer: "Viewer" };
const ROLE_HINTS: Record<Role, string> = {
  admin: "Everything an inspector can do, plus people, computers and the plan.",
  inspector: "Signs GeoGIS in, syncs surveys, sees surveys and reports.",
  viewer: "Sees surveys and reports on the web. Organisation plan only.",
};

const date = (secs: number) => new Date(secs * 1000).toLocaleDateString();
const dateTime = (secs: number) => new Date(secs * 1000).toLocaleString();

/** How many paid seats and viewer seats are taken. */
export function seatsInUse(list: MemberList): { full: number; viewers: number } {
  const viewers = list.members.filter((m) => m.role === "viewer").length;
  return { full: list.members.length - viewers, viewers };
}

export function TeamPage() {
  const me = useMe().data;
  if (me && me.membership.role !== "admin") {
    return (
      <main className="narrow">
        <h1>Team</h1>
        <Notice kind="info">Only admins of {me.organisation.name} can see and change who is in it.</Notice>
      </main>
    );
  }
  return me ? <Team me={me} /> : null;
}

function Team({ me }: { me: Me }) {
  const api = useApi();
  const qc = useQueryClient();
  const members = useQuery({ queryKey: ["members"], queryFn: ({ signal }) => api.members(signal) });
  const done = () => qc.invalidateQueries({ queryKey: ["members"] });
  const setRole = useMutation({ mutationFn: (v: { user: string; role: Role }) => api.setRole(v.user, v.role), onSettled: done });
  const remove = useMutation({ mutationFn: (user: string) => api.removeMember(user), onSettled: done });
  const failure = setRole.error ?? remove.error;

  return (
    <main className="wide">
      <h1>Team</h1>
      {members.isPending ? (
        <Loading />
      ) : members.error ? (
        <Notice kind="error">{describeFailure(members.error)}</Notice>
      ) : (
        <>
          <Seats list={members.data} />
          {failure ? <Notice kind="error">{describeFailure(failure)}</Notice> : null}
          <table className="list">
            <thead>
              <tr>
                <th scope="col">Name</th>
                <th scope="col">Email</th>
                <th scope="col">Role</th>
                <th scope="col">Joined</th>
                <th scope="col">
                  <span className="sr-only">Remove</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {members.data.members.map((m) => {
                const self = m.user.id === me.user.id;
                return (
                  <tr key={m.user.id}>
                    <td>
                      {m.user.name}
                      {self ? <span className="muted"> (you)</span> : null}
                    </td>
                    <td>{m.user.email}</td>
                    <td>
                      <select
                        aria-label={`Role of ${m.user.name}`}
                        value={m.role}
                        disabled={self || setRole.isPending}
                        title={self ? "Another admin can change your role." : undefined}
                        onChange={(e) => setRole.mutate({ user: m.user.id, role: e.target.value as Role })}
                      >
                        {(Object.keys(ROLE_NAMES) as Role[]).map((r) => (
                          <option key={r} value={r}>
                            {ROLE_NAMES[r]}
                          </option>
                        ))}
                      </select>
                    </td>
                    <td>{date(m.joined)}</td>
                    <td>
                      {self ? null : (
                        <button
                          type="button"
                          className="link danger"
                          disabled={remove.isPending}
                          onClick={() => {
                            if (window.confirm(`Remove ${m.user.name} from ${me.organisation.name}? Their computers are signed out too.`)) {
                              remove.mutate(m.user.id);
                            }
                          }}
                        >
                          Remove
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          <InviteForm me={me} list={members.data} />
        </>
      )}
    </main>
  );
}

function Seats({ list }: { list: MemberList }) {
  const used = seatsInUse(list);
  return (
    <p className="muted">
      Inspectors and admins: {used.full} of {list.seats}
      {list.viewerSeats > 0 ? ` · Viewers: ${used.viewers} of ${list.viewerSeats}` : ""}
    </p>
  );
}

function InviteForm({ me, list }: { me: Me; list: MemberList }) {
  const api = useApi();
  const viewersAllowed = me.effectivePlan === "organisation";
  const [role, setRole] = useState<Role>("inspector");
  const [sentTo, setSentTo] = useState<string | null>(null);
  const invite = useMutation({
    mutationFn: (v: { email: string; role: Role }) => api.invite(v.email, v.role),
    onSuccess: (_d, v) => setSentTo(v.email),
  });
  const used = seatsInUse(list);
  const full = role === "viewer" ? used.viewers >= list.viewerSeats : used.full >= list.seats;
  const submit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    const form = e.currentTarget;
    const email = new FormData(form).get("email");
    setSentTo(null);
    invite.mutate({ email: typeof email === "string" ? email : "", role }, { onSuccess: () => form.reset() });
  };
  return (
    <section className="card">
      <h2>Invite someone</h2>
      {sentTo ? (
        <Notice kind="success">
          Invitation sent to <strong>{sentTo}</strong>. The link works for 7 days.
        </Notice>
      ) : null}
      {full ? (
        <Notice kind="info">
          Every {role === "viewer" ? "viewer" : "inspector and admin"} seat on your plan is taken. You can still send the
          invitation, but it can only be accepted once a seat is free or the plan has more seats.
        </Notice>
      ) : null}
      {invite.error && !fieldReason(invite.error, "email") ? <Notice kind="error">{inviteFailure(invite.error)}</Notice> : null}
      <form onSubmit={submit} noValidate className="inline-form">
        <Field label="Email" name="email" type="email" autoComplete="off" required error={reasonText(fieldReason(invite.error, "email"))} />
        <div className="field">
          <label htmlFor="invite-role">Role</label>
          <select id="invite-role" value={role} onChange={(e) => setRole(e.target.value as Role)}>
            {(Object.keys(ROLE_NAMES) as Role[]).map((r) => (
              <option key={r} value={r} disabled={r === "viewer" && !viewersAllowed}>
                {ROLE_NAMES[r]}
                {r === "viewer" && !viewersAllowed ? " (Organisation plan)" : ""}
              </option>
            ))}
          </select>
          <p className="hint">{ROLE_HINTS[role]}</p>
        </div>
        <button type="submit" className="primary" disabled={invite.isPending}>
          {invite.isPending ? "Sending…" : "Send invitation"}
        </button>
      </form>
    </section>
  );
}

function inviteFailure(e: unknown): string {
  if (e instanceof ApiFailure && e.code === "conflict") return "This person already has a GeoFlux account.";
  return describeFailure(e);
}

export function ComputersPage() {
  const api = useApi();
  const qc = useQueryClient();
  const me = useMe().data;
  const admin = me?.membership.role === "admin";
  const devices = useQuery({ queryKey: ["devices"], queryFn: ({ signal }) => api.devices(signal) });
  // Admins see everyone's computers; name the person rather than show an id.
  const members = useQuery({ queryKey: ["members"], queryFn: ({ signal }) => api.members(signal), enabled: admin });
  const revoke = useMutation({
    mutationFn: (id: string) => api.revokeDevice(id),
    onSettled: () => qc.invalidateQueries({ queryKey: ["devices"] }),
  });
  const person = (userId: string) =>
    userId === me?.user.id ? "You" : (members.data?.members.find((m) => m.user.id === userId)?.user.name ?? "Someone who left");
  const now = Date.now() / 1000;
  return (
    <main className="wide">
      <h1>Computers</h1>
      <p className="lead muted">
        {admin
          ? "Every computer signed in to GeoGIS with an account in your organisation."
          : "The computers you've signed in to GeoGIS."}{" "}
        Signing one out ends its sign-in, so it can't sync or renew. A pass it already holds keeps working offline until
        it runs out.
      </p>
      {revoke.error ? <Notice kind="error">{describeFailure(revoke.error)}</Notice> : null}
      {devices.isPending ? (
        <Loading />
      ) : devices.error ? (
        <Notice kind="error">{describeFailure(devices.error)}</Notice>
      ) : devices.data.devices.length === 0 ? (
        <p className="empty">No computers yet. In GeoGIS, open Analyze › Inspection › Account and click Sign In.</p>
      ) : (
        <table className="list">
          <thead>
            <tr>
              <th scope="col">Computer</th>
              {admin ? <th scope="col">Person</th> : null}
              <th scope="col">Signed in</th>
              <th scope="col">Last seen</th>
              <th scope="col" title="An offline pass file was issued for this computer; it can't be withdrawn before it runs out.">
                Offline pass file until
              </th>
              <th scope="col">
                <span className="sr-only">Sign out</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {devices.data.devices.map((d) => (
              <tr key={d.id}>
                <td className="device-name">{d.name}</td>
                {admin ? <td>{person(d.user)}</td> : null}
                <td>{date(d.signedIn)}</td>
                <td>{dateTime(d.lastSeen)}</td>
                <td>{d.offlineUntil ? (d.offlineUntil < now ? <span className="muted">Ran out</span> : date(d.offlineUntil)) : "—"}</td>
                <td>
                  <button
                    type="button"
                    className="link danger"
                    disabled={revoke.isPending}
                    onClick={() => {
                      if (window.confirm(`Sign out GeoGIS on ${d.name}?`)) revoke.mutate(d.id);
                    }}
                  >
                    Sign out
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </main>
  );
}
