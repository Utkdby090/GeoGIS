import { QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter } from "react-router";

import { HttpApi } from "./api/http";
import { App } from "./App";
import { ApiProvider } from "./app/api-context";
import { registerServiceWorker } from "./app/offline";
import { makeQueryClient } from "./app/session";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("index.html has no #root");

createRoot(root).render(
  <StrictMode>
    <ApiProvider api={new HttpApi()}>
      <QueryClientProvider client={makeQueryClient()}>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </QueryClientProvider>
    </ApiProvider>
  </StrictMode>,
);

// Only in a build: the dev server's files change on every edit.
if (import.meta.env.PROD) registerServiceWorker();
