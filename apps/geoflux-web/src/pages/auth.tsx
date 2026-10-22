// Logging in and everything around it: sign-up, forgotten and reset passwords, confirming an
// email address, and joining an organisation from an invitation.

import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Link, Navigate, useNavigate, useSearchParams } from "react-router";

import { describeFailure, fieldReason, hasFieldErrors } from "../api/http";
import { useApi } from "../app/api-context";
import { ME_KEY, safeNext, useMe } from "../app/session";
import { AuthCard, Field, Loading, Notice, PASSWORD_HINT, reasonText } from "../app/ui";

/** The form's values, read by input name, so the browser's autofill is picked up too. */
function values(e: FormEvent<HTMLFormElement>): Record<string, string> {
  e.preventDefault();
  const out: Record<string, string> = {};
  new FormData(e.currentTarget).forEach((v, k) => {
    if (typeof v === "string") out[k] = v;
  });
  return out;
}

function nextLink(next: string): string {
  return next === "/" ? "" : `?next=${encodeURIComponent(next)}`;
}

export function LogInPage() {
  const api = useApi();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const next = safeNext(params.get("next"));
  const me = useMe();
  const logIn = useMutation({
    mutationFn: (v: Record<string, string>) => api.logIn(v["email"] ?? "", v["password"] ?? ""),
    onSuccess: (who) => {
      qc.setQueryData(ME_KEY, who);
      void navigate(next, { replace: true });
    },
  });
  if (me.data && !logIn.isPending) return <Navigate to={next} replace />;
  return (
    <AuthCard
      title="Log in"
      footer={
        <>
          New to GeoFlux? <Link to={`/signup${nextLink(next)}`}>Create an account</Link>
        </>
      }
    >
      {logIn.error ? <Notice kind="error">{describeFailure(logIn.error)}</Notice> : null}
      <form onSubmit={(e) => logIn.mutate(values(e))} noValidate>
        <Field label="Email" name="email" type="email" autoComplete="username" required autoFocus />
        <Field label="Password" name="password" type="password" autoComplete="current-password" required />
        <button type="submit" className="primary" disabled={logIn.isPending}>
          {logIn.isPending ? "Logging in…" : "Log in"}
        </button>
      </form>
      <p className="aside">
        <Link to="/forgot-password">Forgot your password?</Link>
      </p>
    </AuthCard>
  );
}

export function SignUpPage() {
  const api = useApi();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const next = safeNext(params.get("next"));
  const signUp = useMutation({
    mutationFn: (v: Record<string, string>) =>
      api.signUp({
        name: v["name"] ?? "",
        email: v["email"] ?? "",
        organisation: v["organisation"] ?? "",
        password: v["password"] ?? "",
      }),
    onSuccess: (who) => {
      qc.setQueryData(ME_KEY, who);
      void navigate(next, { replace: true });
    },
  });
  const err = (f: string) => reasonText(fieldReason(signUp.error, f));
  return (
    <AuthCard
      title="Create your GeoFlux account"
      footer={
        <>
          Already have an account? <Link to={`/login${nextLink(next)}`}>Log in</Link>
        </>
      }
    >
      <p className="lead">Your organisation starts with a free trial of every feature. No card needed.</p>
      {signUp.error && !hasFieldErrors(signUp.error) ? <Notice kind="error">{describeFailure(signUp.error)}</Notice> : null}
      <form onSubmit={(e) => signUp.mutate(values(e))} noValidate>
        <Field label="Your name" name="name" autoComplete="name" required autoFocus error={err("name")} />
        <Field label="Work email" name="email" type="email" autoComplete="email" required error={err("email")} />
        <Field
          label="Organisation"
          name="organisation"
          autoComplete="organization"
          required
          hint="Your company or team. You can invite colleagues later."
          error={err("organisation")}
        />
        <Field
          label="Password"
          name="password"
          type="password"
          autoComplete="new-password"
          required
          minLength={10}
          hint={PASSWORD_HINT}
          error={err("password")}
        />
        <button type="submit" className="primary" disabled={signUp.isPending}>
          {signUp.isPending ? "Creating your account…" : "Create account"}
        </button>
      </form>
    </AuthCard>
  );
}

export function ForgotPasswordPage() {
  const api = useApi();
  const forgot = useMutation({ mutationFn: (email: string) => api.forgotPassword(email) });
  return (
    <AuthCard
      title="Reset your password"
      footer={
        <>
          Remembered it? <Link to="/login">Log in</Link>
        </>
      }
    >
      {forgot.isSuccess ? (
        <Notice kind="success">
          If an account uses that address, we've sent it a link to choose a new password. The link works for one hour.
        </Notice>
      ) : (
        <>
          {forgot.error ? <Notice kind="error">{describeFailure(forgot.error)}</Notice> : null}
          <form onSubmit={(e) => forgot.mutate(values(e)["email"] ?? "")} noValidate>
            <Field label="Email" name="email" type="email" autoComplete="username" required autoFocus />
            <button type="submit" className="primary" disabled={forgot.isPending}>
              {forgot.isPending ? "Sending…" : "Send me a link"}
            </button>
          </form>
        </>
      )}
    </AuthCard>
  );
}

export function ResetPasswordPage() {
  const api = useApi();
  const qc = useQueryClient();
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const [mismatch, setMismatch] = useState(false);
  const reset = useMutation({
    mutationFn: (password: string) => api.resetPassword(token, password),
    // Every session of this account ends with a reset, this one included.
    onSuccess: () => qc.setQueryData(ME_KEY, null),
  });
  if (!token) return <BadLink title="Reset your password" again={<Link to="/forgot-password">ask for a new link</Link>} />;
  return (
    <AuthCard title="Choose a new password">
      {reset.isSuccess ? (
        <Notice kind="success">
          Your password is changed and you've been logged out everywhere. <Link to="/login">Log in</Link> with the new one.
        </Notice>
      ) : (
        <>
          {reset.error && !hasFieldErrors(reset.error) ? (
            <Notice kind="error">
              {describeFailure(reset.error)} <Link to="/forgot-password">Ask for a new link.</Link>
            </Notice>
          ) : null}
          <form
            onSubmit={(e) => {
              const v = values(e);
              const same = v["password"] === v["confirm"];
              setMismatch(!same);
              if (same) reset.mutate(v["password"] ?? "");
            }}
            noValidate
          >
            <Field
              label="New password"
              name="password"
              type="password"
              autoComplete="new-password"
              required
              autoFocus
              hint={PASSWORD_HINT}
              error={reasonText(fieldReason(reset.error, "password"))}
            />
            <Field
              label="Type it again"
              name="confirm"
              type="password"
              autoComplete="new-password"
              required
              error={mismatch ? "The two passwords are different." : null}
            />
            <button type="submit" className="primary" disabled={reset.isPending}>
              {reset.isPending ? "Saving…" : "Save new password"}
            </button>
          </form>
        </>
      )}
    </AuthCard>
  );
}

export function VerifyEmailPage() {
  const api = useApi();
  const qc = useQueryClient();
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const verify = useMutation({
    mutationFn: () => api.verifyEmail(token),
    onSuccess: () => qc.invalidateQueries({ queryKey: ME_KEY }),
  });
  // Confirm as soon as the link is opened; the ref keeps a development double render from
  // sending it twice (the second would fail, the token being used up).
  const sent = useRef(false);
  useEffect(() => {
    if (token && !sent.current) {
      sent.current = true;
      verify.mutate();
    }
  }, [token, verify]);
  if (!token) return <BadLink title="Confirm your email" again={<Link to="/">log in to get a new one</Link>} />;
  return (
    <AuthCard title="Confirm your email">
      {verify.isSuccess ? (
        <Notice kind="success">
          Your email address is confirmed. <Link to="/">Go to GeoFlux</Link>
        </Notice>
      ) : verify.error ? (
        <Notice kind="error">
          {describeFailure(verify.error)} Log in and we'll offer to send a new link.
        </Notice>
      ) : (
        <Loading what="Confirming" />
      )}
    </AuthCard>
  );
}

export function InvitePage() {
  const api = useApi();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const token = params.get("token") ?? "";
  const accept = useMutation({
    mutationFn: (v: Record<string, string>) => api.acceptInvite(token, v["name"] ?? "", v["password"] ?? ""),
    onSuccess: async () => {
      // Joining signs the person in; read who they are now.
      await qc.invalidateQueries({ queryKey: ME_KEY });
      void navigate("/", { replace: true });
    },
  });
  if (!token) return <BadLink title="Join your team on GeoFlux" again={<span>ask your admin to invite you again</span>} />;
  const err = (f: string) => reasonText(fieldReason(accept.error, f));
  return (
    <AuthCard
      title="Join your team on GeoFlux"
      footer={
        <>
          Already have an account? <Link to="/login">Log in</Link>
        </>
      }
    >
      <p className="lead">Choose your name and a password to accept the invitation.</p>
      {accept.error && !hasFieldErrors(accept.error) ? (
        <Notice kind="error">{describeFailure(accept.error)}</Notice>
      ) : null}
      <form onSubmit={(e) => accept.mutate(values(e))} noValidate>
        <Field label="Your name" name="name" autoComplete="name" required autoFocus error={err("name")} />
        <Field
          label="Password"
          name="password"
          type="password"
          autoComplete="new-password"
          required
          hint={PASSWORD_HINT}
          error={err("password")}
        />
        <button type="submit" className="primary" disabled={accept.isPending}>
          {accept.isPending ? "Joining…" : "Join"}
        </button>
      </form>
    </AuthCard>
  );
}

function BadLink({ title, again }: { title: string; again: ReactNode }) {
  return (
    <AuthCard title={title}>
      <Notice kind="error">This link is incomplete. Open it again from the email, or {again}.</Notice>
    </AuthCard>
  );
}
