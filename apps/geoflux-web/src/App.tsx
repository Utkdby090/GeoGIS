// Every page and its address. The addresses in PAGES (client.ts) are linked from emails and from
// GeoGIS, so they must not change.

import { Link, Route, Routes } from "react-router";

import { PAGES } from "./api/client";
import { AppLayout, RequireSignIn } from "./app/layout";
import { AuthCard } from "./app/ui";
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
