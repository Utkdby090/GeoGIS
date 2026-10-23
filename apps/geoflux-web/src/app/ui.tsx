// Small building blocks shared by the pages. Plain HTML elements with labels, so the forms work
// with a keyboard, a screen reader and the browser's password manager.

import { useId, type InputHTMLAttributes, type ReactNode } from "react";
import { Link } from "react-router";

/** The server's field reasons (geoflux_cloud::auth), as a person reads them. */
const REASONS: Record<string, string> = {
  required: "Fill this in.",
  invalid: "This doesn't look right.",
  not_an_email: "Enter an email address, like name@company.com.",
  too_short: "Use at least 10 characters.",
  too_long: "Use at most 128 characters.",
  same_as_email: "Don't use your email address as your password.",
  too_simple: "Use something harder to guess.",
};

export function reasonText(reason: string | null): string | null {
  return reason === null ? null : (REASONS[reason] ?? "This doesn't look right.");
}

type FieldProps = Omit<InputHTMLAttributes<HTMLInputElement>, "id"> & {
  label: string;
  hint?: string;
  error?: string | null;
};

export function Field({ label, hint, error, ...input }: FieldProps) {
  const id = useId();
  const described = [hint ? `${id}-hint` : null, error ? `${id}-error` : null].filter(Boolean).join(" ");
  return (
    <div className="field">
      <label htmlFor={id}>{label}</label>
      <input id={id} aria-invalid={error ? true : undefined} aria-describedby={described || undefined} {...input} />
      {hint ? (
        <p className="hint" id={`${id}-hint`}>
          {hint}
        </p>
      ) : null}
      {error ? (
        <p className="field-error" id={`${id}-error`}>
          {error}
        </p>
      ) : null}
    </div>
  );
}

export function Notice({ kind = "info", children }: { kind?: "info" | "error" | "success"; children: ReactNode }) {
  return (
    <div className={`notice notice-${kind}`} role={kind === "error" ? "alert" : "status"}>
      {children}
    </div>
  );
}

export function Loading({ what = "Loading" }: { what?: string }) {
  return (
    <p className="loading" role="status">
      {what}…
    </p>
  );
}

/** The centred card the sign-in pages sit in. */
export function AuthCard({ title, children, footer }: { title: string; children: ReactNode; footer?: ReactNode }) {
  return (
    <main className="auth">
      <div className="auth-card">
        <Link to="/" className="brand">
          GeoFlux
        </Link>
        <h1>{title}</h1>
        {children}
      </div>
      {footer ? <div className="auth-footer">{footer}</div> : null}
    </main>
  );
}

export const PASSWORD_HINT = "At least 10 characters. A few unrelated words work well.";
