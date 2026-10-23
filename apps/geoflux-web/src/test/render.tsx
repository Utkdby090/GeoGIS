import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import { MemoryRouter, useLocation } from "react-router";

import type { GeoFluxApi } from "../api/client";
import { App } from "../App";
import { ApiProvider } from "../app/api-context";
import { makeQueryClient } from "../app/session";

/** Where the app is now, shown for assertions. */
function Here() {
  const l = useLocation();
  return <output data-testid="here">{`${l.pathname}${l.search}`}</output>;
}

export function renderApp(api: GeoFluxApi, at: string) {
  const client = makeQueryClient();
  return render(
    <ApiProvider api={api}>
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[at]}>
          <App />
          <Here />
        </MemoryRouter>
      </QueryClientProvider>
    </ApiProvider>,
  );
}
