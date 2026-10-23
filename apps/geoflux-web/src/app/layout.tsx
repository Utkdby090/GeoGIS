// The frame of the signed-in pages: the header with the organisation and the person, and the
// guard that sends anyone not signed in to the log-in page and back afterwards.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Link, Navigate, NavLink, Outlet, useLocation, useNavigate } from "react-router";

import type { Me } from "../api/schemas";
import { describeFailure } from "../api/http";
import { useApi } from "./api-context";
import { forgetAccountData, useOnline } from "./offline";
import { ME_KEY, hereAsNext, useMe } from "./session";
import { Loading, Notice } from "./ui";

/** Renders its children only for a signed-in person. */
export function RequireSignIn({ children }: { children: ReactNode }) {
  const me = useMe();
  const location = useLocation();
  if (me.isPending) return <Loading />;
  if (me.error) {
    return (
      <main className="narrow">
        <Notice kind="error">{describeFailure(me.error)}</Notice>
        <button type="button" onClick={() => void me.refetch()}>
          Try again
        </button>
      </main>
    );
  }
  if (!me.data) return <Navigate to={`/login?next=${encodeURIComponent(hereAsNext(location))}`} replace />;
  return children;
}

/** Logs out, forgets everything the app had loaded, and goes to the log-in page with `next`. */
export function useLogOut() {
  const api = useApi();
  const qc = useQueryClient();
  const navigate = useNavigate();
  return useMutation({
    mutationFn: () => api.logOut(),
    onSettled: async (_data, _error, next: string | undefined) => {
      await forgetAccountData();
      // Even if the server couldn't be told, nothing of this account stays in the page.
      qc.clear();
      qc.setQueryData(ME_KEY, null);
      void navigate(next ? `/login?next=${encodeURIComponent(next)}` : "/login", { replace: true });
    },
  });
}

export function AppLayout() {
  const me = useMe().data;
  const logOut = useLogOut();
  const online = useOnline();
  if (!me) return null;
  return (
    <div className="app">
      <header className="top">
        <Link to="/" className="brand">
          GeoFlux
        </Link>
        <nav>
          <NavLink to="/" end>
            Surveys
          </NavLink>
          <NavLink to="/computers">Computers</NavLink>
          {me.membership.role === "admin" ? <NavLink to="/team">Team</NavLink> : null}
        </nav>
        <span className="who">
          <span className="org">{me.organisation.name}</span>
          <span className="plan">{planText(me, Date.now() / 1000)}</span>
          <span className="user" title={me.user.email}>
            {me.user.name}
          </span>
          <button type="button" className="link" disabled={logOut.isPending} onClick={() => logOut.mutate(undefined)}>
            Log out
          </button>
        </span>
      </header>
      {!online ? (
        <Notice kind="info">
          You're offline. GeoFlux shows what this browser saved the last time it was online; changes and downloads wait
          for the connection.
        </Notice>
      ) : null}
      {!me.user.emailVerified ? (
        <Notice kind="info">
          Confirm your email address: we sent a link to <strong>{me.user.email}</strong>. Until then GeoGIS can't be
          signed in with this account.
        </Notice>
      ) : null}
      <Outlet />
    </div>
  );
}

const PLAN_NAMES = { free: "Free", pro: "Pro", organisation: "Organisation", unknown: "Free" } as const;

/** "Pro trial, 12 days left", "Organisation", or "Free" once a subscription has lapsed. */
export function planText(me: Me, nowSecs: number): string {
  const name = PLAN_NAMES[me.subscription.plan];
  const s = me.subscription.state;
  switch (s.state) {
    case "trial": {
      const days = Math.max(0, Math.ceil((s.ends - nowSecs) / 86_400));
      return `${name} trial, ${days} day${days === 1 ? "" : "s"} left`;
    }
    case "active":
      return name;
    case "lapsed":
      return PLAN_NAMES[me.effectivePlan];
  }
}
