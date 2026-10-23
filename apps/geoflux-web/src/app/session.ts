// Who is signed in, kept in the query cache under ME_KEY. Any call that finds the session gone
// (it ran out, or an admin removed the person) sets it to null, and the signed-in pages send the
// person to the log-in page.

import { QueryCache, QueryClient, useQuery } from "@tanstack/react-query";

import { ApiFailure } from "../api/client";
import type { Me } from "../api/schemas";
import { useApi } from "./api-context";

export const ME_KEY = ["me"] as const;

export function useMe() {
  const api = useApi();
  return useQuery<Me | null>({ queryKey: ME_KEY, queryFn: ({ signal }) => api.me(signal), staleTime: 60_000 });
}

export function makeQueryClient(): QueryClient {
  const client: QueryClient = new QueryClient({
    queryCache: new QueryCache({
      onError: (e, query) => {
        if (e instanceof ApiFailure && e.signedOut && query.queryKey[0] !== ME_KEY[0]) {
          client.setQueryData(ME_KEY, null);
        }
      },
    }),
    defaultOptions: {
      queries: {
        // Only a dropped connection is worth trying again; the server's own "no" won't change.
        retry: (count, e) => e instanceof ApiFailure && e.kind === "network" && count < 2,
        refetchOnWindowFocus: false,
      },
      mutations: { retry: false },
    },
  });
  return client;
}

/**
 * Where to go after logging in. Only a path on this site: `//evil.example` or `/\evil.example`
 * would leave it, and so would anything with a scheme.
 */
export function safeNext(next: string | null | undefined): string {
  if (!next || !next.startsWith("/") || next.startsWith("//") || next.startsWith("/\\")) return "/";
  // Control characters (a tab or newline) are dropped by URL parsing, which could turn
  // "/\t/evil.example" into "//evil.example".
  if ([...next].some((c) => c.charCodeAt(0) < 0x20 || c.charCodeAt(0) === 0x7f)) return "/";
  return next;
}

/** The current page as a `next` value. */
export function hereAsNext(location: { pathname: string; search: string }): string {
  return `${location.pathname}${location.search}`;
}
