// The server's routes, the same table as `geoflux_cloud::api::routes` (contract.test.ts checks them
// against fixtures/routes.json, which the Rust side writes).

export const ROUTES = {
  SIGN_UP: ["POST", "/api/v1/auth/signup"],
  LOG_IN: ["POST", "/api/v1/auth/login"],
  LOG_OUT: ["POST", "/api/v1/auth/logout"],
  FORGOT_PASSWORD: ["POST", "/api/v1/auth/password/forgot"],
  RESET_PASSWORD: ["POST", "/api/v1/auth/password/reset"],
  VERIFY_EMAIL: ["POST", "/api/v1/auth/email/verify"],
  ME: ["GET", "/api/v1/me"],
  DEVICE_APPROVE: ["POST", "/api/v1/device/approve"],
  DEVICE_PENDING: ["GET", "/api/v1/device/pending"],
  DEVICES: ["GET", "/api/v1/devices"],
  REVOKE_DEVICE: ["DELETE", "/api/v1/devices/{id}"],
  MEMBERS: ["GET", "/api/v1/org/members"],
  INVITE: ["POST", "/api/v1/org/invites"],
  ACCEPT_INVITE: ["POST", "/api/v1/invites/accept"],
  SET_ROLE: ["PATCH", "/api/v1/org/members/{id}"],
  REMOVE_MEMBER: ["DELETE", "/api/v1/org/members/{id}"],
  SURVEYS: ["GET", "/api/v1/surveys"],
  SURVEY: ["GET", "/api/v1/surveys/{id}"],
  SURVEY_TOWERS: ["GET", "/api/v1/surveys/{id}/towers"],
  SURVEY_ASSET: ["GET", "/api/v1/surveys/{id}/assets/{asset}"],
  PHOTO: ["GET", "/api/v1/surveys/{id}/photos/{sha256}"],
  TOWER_REPORT: ["GET", "/api/v1/surveys/{id}/assets/{asset}/report.pdf"],
  LINE_REPORT: ["GET", "/api/v1/surveys/{id}/report.zip"],
  SURVEY_HEALTH: ["GET", "/api/v1/surveys/{id}/health"],
  TOWER_HEALTH: ["GET", "/api/v1/surveys/{id}/assets/{asset}/health"],
  SCHEMA: ["GET", "/api/v1/schemas/{id}"],
} as const satisfies Record<string, readonly [string, string]>;

export type RouteName = keyof typeof ROUTES;

/**
 * A route's path with its `{name}` parameters filled in, each percent-encoded, so a tower id like
 * `T/12 ?` can't change which route is called. `.` and `..` are encoded too, or the browser would
 * resolve them as "this folder" and "the folder above" before sending.
 */
export function path(route: RouteName, params: Record<string, string> = {}): string {
  const template: string = ROUTES[route][1];
  return template.replace(/\{([a-z0-9]+)\}/gi, (_, name: string) => {
    const value = params[name];
    if (value === undefined || value === "") throw new Error(`route ${route} needs ${name}`);
    const encoded = encodeURIComponent(value);
    return encoded === "." || encoded === ".." ? encoded.replaceAll(".", "%2E") : encoded;
  });
}
