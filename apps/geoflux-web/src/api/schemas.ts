// The server's JSON, as the web app reads it. Every response is parsed with these schemas before
// the app touches it, so a wrong or tampered body becomes one clear error instead of a broken page.
//
// They mirror the Rust types in crates/geoflux-cloud (api.rs, model.rs, sync.rs, view.rs) and
// geoflux-domain. The fixtures in ./fixtures are written by that crate's `web_fixtures` test from the
// real types, and contract.test.ts parses each one here, so the two sides can't drift apart.
//
// The app is built into the server binary, so both always ship together: enums are strict.
// Objects ignore keys they don't know, so the server can add a field before the app uses it.

import { z } from "zod";

// ---- Shared ----------------------------------------------------------------------------------

/** Seconds since 1970, as the server sends every time. */
export const UnixSeconds = z.number().int();

const id = (prefix: string) => z.string().regex(new RegExp(`^${prefix}_[0-9a-f]{32}$`));
export const OrgId = id("org");
export const UserId = id("usr");
export const DeviceId = id("dev");
export const SurveyId = id("srv");
export const Sha256 = z.string().regex(/^[0-9a-f]{64}$/);

export const Severity = z.enum(["none", "low", "medium", "high", "critical"]);
export type Severity = z.infer<typeof Severity>;
/** In the order the server counts them (`severityCounts`). */
export const SEVERITIES: readonly Severity[] = Severity.options;

export const RecordStatus = z.enum(["draft", "ready", "archived"]);
export const Role = z.enum(["admin", "inspector", "viewer"]);
export type Role = z.infer<typeof Role>;
/** The server reads a plan it doesn't know as `unknown`; so does the app (never as a paid plan). */
export const Plan = z.enum(["free", "pro", "organisation", "unknown"]).catch("unknown");

// ---- Errors ----------------------------------------------------------------------------------

export const ErrorCode = z.enum([
  "bad_request",
  "invalid_field",
  "unauthenticated",
  "invalid_credentials",
  "email_not_verified",
  "forbidden",
  "plan_required",
  "seat_limit",
  "revoked",
  "not_found",
  "conflict",
  "incomplete",
  "too_large",
  "unsupported_media_type",
  "rate_limited",
  "authorization_pending",
  "slow_down",
  "access_denied",
  "expired_token",
  "internal",
  "unavailable",
]);
export type ErrorCode = z.infer<typeof ErrorCode>;

export const FieldError = z.object({ field: z.string(), reason: z.string() });

/** The error body of every route. */
export const ApiErrorBody = z.object({
  error: ErrorCode,
  message: z.string(),
  fields: z.array(FieldError).optional(),
  devices: z.number().int().optional(),
  missing: z.array(Sha256).optional(),
  retryAfter: z.number().int().optional(),
});
export type ApiErrorBody = z.infer<typeof ApiErrorBody>;

// ---- People and organisations ----------------------------------------------------------------

export const User = z.object({
  id: UserId,
  email: z.string(),
  name: z.string(),
  emailVerified: z.boolean(),
  created: UnixSeconds,
});

export const Organisation = z.object({ id: OrgId, name: z.string(), created: UnixSeconds });

export const Membership = z.object({ user: UserId, org: OrgId, role: Role, joined: UnixSeconds });

export const SubscriptionState = z.discriminatedUnion("state", [
  z.object({ state: z.literal("trial"), ends: UnixSeconds }),
  z.object({ state: z.literal("active"), until: UnixSeconds }),
  z.object({ state: z.literal("lapsed"), since: UnixSeconds }),
]);

export const Subscription = z.object({
  org: OrgId,
  plan: Plan,
  state: SubscriptionState,
  billing: z.enum(["trial", "invoice", "online"]),
  seats: z.number().int(),
  viewerSeats: z.number().int(),
});

/** `GET /me`: who is signed in. */
export const Me = z.object({
  user: User,
  organisation: Organisation,
  membership: Membership,
  subscription: Subscription,
  effectivePlan: Plan,
});
export type Me = z.infer<typeof Me>;

export const Member = z.object({ user: User, role: Role, joined: UnixSeconds });
export const MemberList = z.object({
  members: z.array(Member),
  seats: z.number().int(),
  viewerSeats: z.number().int(),
});
export type MemberList = z.infer<typeof MemberList>;

export const Device = z.object({
  id: DeviceId,
  org: OrgId,
  user: UserId,
  machine: z.string(),
  name: z.string(),
  signedIn: UnixSeconds,
  lastSeen: UnixSeconds,
  offlineUntil: UnixSeconds.nullable(),
});
export const DeviceList = z.object({ devices: z.array(Device) });
export type DeviceList = z.infer<typeof DeviceList>;

/** `GET /device/pending?code=`: what approving a GeoGIS sign-in code would sign in. */
export const PendingDevice = z.object({ deviceName: z.string(), expires: UnixSeconds });
export type PendingDevice = z.infer<typeof PendingDevice>;

// ---- Surveys ---------------------------------------------------------------------------------

export const SurveySummary = z.object({
  id: SurveyId,
  org: OrgId,
  clientSurveyId: z.string(),
  revision: z.number().int().positive(),
  project: z.string(),
  line: z.string().nullable(),
  schemaId: z.string(),
  state: z.enum(["uploading", "committed"]),
  uploadedBy: UserId,
  uploaded: UnixSeconds,
  committed: UnixSeconds.nullable(),
  assets: z.number().int(),
  photos: z.number().int(),
  photoBytes: z.number().int(),
  /** Towers per severity, in SEVERITIES order. */
  severityCounts: z.tuple([z.number(), z.number(), z.number(), z.number(), z.number()]),
});
export type SurveySummary = z.infer<typeof SurveySummary>;

export const SurveyList = z.object({ surveys: z.array(SurveySummary) });

/** `[west, south, east, north]`. */
export const Bounds = z.tuple([z.number(), z.number(), z.number(), z.number()]);

export const SurveyDetail = z.object({
  summary: SurveySummary,
  bounds: Bounds.nullable(),
  /** Every upload of the same survey, newest first. */
  revisions: z.array(z.object({ id: SurveyId, revision: z.number().int(), committed: UnixSeconds.nullable() })),
});
export type SurveyDetail = z.infer<typeof SurveyDetail>;

// ---- The map ---------------------------------------------------------------------------------

/** `GET /surveys/{id}/towers`: GeoJSON, longitude first, in walking order. */
export const TowerFeature = z.object({
  type: z.literal("Feature"),
  id: z.string(),
  geometry: z.object({ type: z.literal("Point"), coordinates: z.tuple([z.number(), z.number()]) }).nullable(),
  properties: z.object({
    label: z.string(),
    position: z.number().int(),
    severity: Severity,
    status: RecordStatus,
    photos: z.number().int(),
  }),
});
export type TowerFeature = z.infer<typeof TowerFeature>;

export const TowerCollection = z.object({ type: z.literal("FeatureCollection"), features: z.array(TowerFeature) });
export type TowerCollection = z.infer<typeof TowerCollection>;

// ---- One tower -------------------------------------------------------------------------------

/** 0..1 across and down the photo, from its top left. */
const NormPoint = z.object({ x: z.number(), y: z.number() });

/** One markup shape (geoflux_domain::annotation), drawn over the photo; the photo is never changed. */
export const Annotation = z
  .discriminatedUnion("kind", [
    z.object({ kind: z.literal("pen"), points: z.array(NormPoint) }),
    z.object({ kind: z.literal("arrow"), from: NormPoint, to: NormPoint }),
    z.object({ kind: z.literal("rect"), a: NormPoint, b: NormPoint }),
    z.object({ kind: z.literal("ellipse"), a: NormPoint, b: NormPoint }),
    z.object({ kind: z.literal("text"), at: NormPoint, text: z.string(), size: z.number() }),
    z.object({ kind: z.literal("marker"), at: NormPoint, number: z.number().int() }),
  ])
  .and(
    z.object({
      color: z.string().regex(/^#[0-9a-f]{6}([0-9a-f]{2})?$/i),
      /** Thousandths of the photo's shorter side. */
      width: z.number(),
    }),
  );
export type Annotation = z.infer<typeof Annotation>;

export const PhotoRef = z.object({
  /** The schema's image slot id (`THERMAL`, `STRUCTURE`…). */
  slot: z.string(),
  sha256: Sha256,
  bytes: z.number().int(),
  type: z.enum(["jpeg", "png"]),
  /** The original file name, shown as text only. */
  fileName: z.string(),
  markup: z.array(Annotation).optional(),
});
export type PhotoRef = z.infer<typeof PhotoRef>;

export const Asset = z.object({
  id: z.string(),
  schemaId: z.string(),
  position: z.number().int(),
  status: RecordStatus,
  location: z.object({ lat: z.number(), lon: z.number() }).nullable(),
  /** Form values by field key, in the order they were added. */
  properties: z.array(z.tuple([z.string(), z.string()])),
  severity: Severity,
});

/** `GET /surveys/{id}/assets/{asset}`. */
export const TowerDetail = z.object({ survey: SurveyId, asset: Asset, photos: z.array(PhotoRef) });
export type TowerDetail = z.infer<typeof TowerDetail>;

// ---- Schemas (the form's fields) -------------------------------------------------------------

export const FieldDef = z.object({
  key: z.string(),
  label: z.string(),
  type: z.enum(["text", "number", "integer", "date", "dropdown", "multiSelect", "boolean", "photo"]),
  required: z.boolean(),
  group: z.string(),
  options: z.array(z.string()).optional(),
  min: z.number().optional(),
  max: z.number().optional(),
});

export const ImageSlotDef = z.object({ id: z.string(), label: z.string(), thermal: z.boolean() });

/** `GET /schemas/{id}`: only what the web app shows; the rest of the schema is ignored. */
export const DomainSchema = z.object({
  id: z.string(),
  name: z.string(),
  version: z.string(),
  labelField: z.string().optional(),
  fields: z.array(FieldDef),
  imageSlots: z.array(ImageSlotDef),
});
export type DomainSchema = z.infer<typeof DomainSchema>;
