// The GeoFluxApi the pages use, handed down by context so tests can render a page with a fake.

import { createContext, useContext, type ReactNode } from "react";

import type { GeoFluxApi } from "../api/client";

const ApiContext = createContext<GeoFluxApi | null>(null);

export function ApiProvider({ api, children }: { api: GeoFluxApi; children: ReactNode }) {
  return <ApiContext value={api}>{children}</ApiContext>;
}

export function useApi(): GeoFluxApi {
  const api = useContext(ApiContext);
  if (!api) throw new Error("useApi outside ApiProvider");
  return api;
}
