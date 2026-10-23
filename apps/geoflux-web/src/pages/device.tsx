// /device: approving GeoGIS's sign-in code. GeoGIS shows a code like WDJB-MJHT and opens this page
// with it; the person checks that the computer named here is theirs and approves. Until they do,
// GeoGIS keeps asking the server and gets nothing.
//
// The page names the computer before anything is approved, so a code someone else sent can't
// quietly sign a stranger's computer into this account.

import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useSearchParams } from "react-router";

import { ApiFailure } from "../api/client";
import { describeFailure } from "../api/http";
import { useApi } from "../app/api-context";
import { useLogOut } from "../app/layout";
import { useMe } from "../app/session";
import { Field, Loading, Notice } from "../app/ui";

/** As the person typed it: spaces dropped, upper case, and a dash after four letters. */
export function tidyCode(text: string): string {
  const letters = text.toUpperCase().replace(/[^A-Z]/g, "");
  return letters.length === 8 ? `${letters.slice(0, 4)}-${letters.slice(4)}` : letters;
}

export function DevicePage() {
  const [params, setParams] = useSearchParams();
  const code = tidyCode(params.get("code") ?? "");
  return (
    <main className="narrow">
      <h1>Sign in GeoGIS</h1>
      {code ? (
        <Pending code={code} onOther={() => setParams({}, { replace: true })} />
      ) : (
        <EnterCode onCode={(c) => setParams({ code: c }, { replace: true })} />
      )}
    </main>
  );
}

function EnterCode({ onCode }: { onCode: (code: string) => void }) {
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        const v = new FormData(e.currentTarget).get("code");
        const c = typeof v === "string" ? tidyCode(v) : "";
        if (c) onCode(c);
      }}
    >
      <p className="lead">Enter the code GeoGIS shows in its Account window.</p>
      <Field
        label="Code"
        name="code"
        autoComplete="off"
        autoCapitalize="characters"
        spellCheck={false}
        placeholder="WDJB-MJHT"
        required
        autoFocus
      />
      <button type="submit" className="primary">
        Continue
      </button>
    </form>
  );
}

function Pending({ code, onOther }: { code: string; onOther: () => void }) {
  const api = useApi();
  const me = useMe().data;
  const pending = useQuery({
    queryKey: ["device-pending", code],
    queryFn: ({ signal }) => api.pendingDevice(code, signal),
    // An answer is only good once; never show a stale "waiting" for a code already answered.
    staleTime: 0,
    gcTime: 0,
  });
  const answer = useMutation({ mutationFn: (approve: boolean) => api.answerDevice(code, approve) });
  const logOut = useLogOut();

  const other = (
    <button type="button" className="link" onClick={onOther}>
      Enter a different code
    </button>
  );

  if (answer.isSuccess) {
    return answer.variables ? (
      <Notice kind="success">
        Done. Go back to GeoGIS; it signs in within a few seconds. You can close this page.
      </Notice>
    ) : (
      <Notice kind="info">Denied. GeoGIS on that computer won't be signed in.</Notice>
    );
  }
  if (pending.isPending) return <Loading what="Checking the code" />;
  if (pending.error) return <Notice kind="error">{describeFailure(pending.error)}</Notice>;
  const device = pending.data;
  if (!device) {
    return (
      <>
        <Notice kind="error">
          The code {code} is not valid or has run out. In GeoGIS, close the Account window and click Sign In again.
        </Notice>
        <p>{other}</p>
      </>
    );
  }
  const until = new Date(device.expires * 1000).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return (
    <>
      <p className="lead">
        GeoGIS on <strong className="device-name">{device.deviceName}</strong> wants to sign in to{" "}
        <strong>{me?.organisation.name}</strong> as <strong>{me?.user.email}</strong>.
      </p>
      <p className="code" aria-label="Code">
        {code}
      </p>
      <p className="hint">
        Check that GeoGIS shows this same code. Only approve it if you started signing in on that computer just now. The
        code works until {until}.
      </p>
      {answer.error ? <Notice kind="error">{answerFailure(answer.error)}</Notice> : null}
      <div className="actions">
        <button type="button" className="primary" disabled={answer.isPending} onClick={() => answer.mutate(true)}>
          Approve
        </button>
        <button type="button" disabled={answer.isPending} onClick={() => answer.mutate(false)}>
          Deny
        </button>
      </div>
      <p className="aside">
        Not you?{" "}
        <button type="button" className="link" onClick={() => logOut.mutate(`/device?code=${encodeURIComponent(code)}`)}>
          Log in as someone else
        </button>{" "}
        · {other}
      </p>
    </>
  );
}

function answerFailure(e: unknown): string {
  if (e instanceof ApiFailure && e.code === "forbidden") {
    return "Viewers can't sign GeoGIS in. Ask an admin of your organisation to make you an inspector.";
  }
  return describeFailure(e);
}
