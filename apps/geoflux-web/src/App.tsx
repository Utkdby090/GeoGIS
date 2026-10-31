// Every page and its address. The addresses in PAGES (client.ts) are linked from emails and from
// GeoGIS, so they must not change.

import { lazy, Suspense } from "react";
import { Link, Route, Routes } from "react-router";

import { PAGES } from "./api/client";
import { AppLayout, RequireSignIn } from "./app/layout";
import { AuthCard, Loading } from "./app/ui";
import {
  ForgotPasswordPage,
  InvitePage,
  LogInPage,
  ResetPasswordPage,
  SignUpPage,
  VerifyEmailPage,
} from "./pages/auth";
import { DevicePage } from "./pages/device";
import { SurveysPage } from "./pages/surveys";
import { ComputersPage, TeamPage } from "./pages/team";
import { SurveyPage } from "./survey/SurveyPage";

// The dashboard brings its own grid and chart code, loaded only when someone opens it.
const HealthPage = lazy(() => import("./health/HealthPage").then((m) => ({ default: m.HealthPage })));

export function App() {
  return (
    <Routes>
      <Route path="/login" element={<LogInPage />} />
      <Route path="/signup" element={<SignUpPage />} />
      <Route path="/forgot-password" element={<ForgotPasswordPage />} />
      <Route path={PAGES.resetPassword} element={<ResetPasswordPage />} />
      <Route path={PAGES.verifyEmail} element={<VerifyEmailPage />} />
      <Route path={PAGES.invite} element={<InvitePage />} />
      <Route
        element={
          <RequireSignIn>
            <AppLayout />
          </RequireSignIn>
        }
      >
        <Route index element={<SurveysPage />} />
        <Route path="/surveys/:id" element={<SurveyPage />} />
        <Route
          path="/surveys/:id/health"
          element={
            <Suspense fallback={<Loading what="Loading the dashboard" />}>
              <HealthPage />
            </Suspense>
          }
        />
        <Route path="/team" element={<TeamPage />} />
        <Route path="/computers" element={<ComputersPage />} />
        <Route path={PAGES.device} element={<DevicePage />} />
      </Route>
      <Route path="*" element={<NotFound />} />
    </Routes>
  );
}

function NotFound() {
  return (
    <AuthCard title="Page not found">
      <p>
        There's nothing at this address. <Link to="/">Go to your surveys</Link>
      </p>
    </AuthCard>
  );
}
