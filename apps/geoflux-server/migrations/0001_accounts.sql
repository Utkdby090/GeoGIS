-- GeoFlux Cloud: organisations, people, sign-in tokens, computers and surveys.
-- Times are seconds since 1970-01-01 UTC (bigint), as in the desktop pass. Enum values are the API's
-- own names (`admin`, `trial`, `jpeg`), so rows read the same as the JSON.

CREATE TABLE organisations (
    id      text PRIMARY KEY,
    name    text NOT NULL,
    created bigint NOT NULL
);

CREATE TABLE subscriptions (
    org          text PRIMARY KEY REFERENCES organisations (id) ON DELETE CASCADE,
    plan         text NOT NULL,
    -- trial / active / lapsed, and the time that goes with it (ends / until / since).
    state        text NOT NULL,
    state_at     bigint NOT NULL,
    billing      text NOT NULL,
    seats        bigint NOT NULL CHECK (seats >= 0),
    viewer_seats bigint NOT NULL CHECK (viewer_seats >= 0)
);

CREATE TABLE users (
    id             text PRIMARY KEY,
    email          text NOT NULL CONSTRAINT users_email_key UNIQUE,
    name           text NOT NULL,
    email_verified boolean NOT NULL,
    created        bigint NOT NULL,
    -- Argon2id in PHC form; NULL for single sign-on only.
    password_hash  text,
    disabled       boolean NOT NULL DEFAULT false
);

-- One organisation per person in this version.
CREATE TABLE memberships (
    user_id text PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    org     text NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    role    text NOT NULL,
    joined  bigint NOT NULL
);
CREATE INDEX memberships_org ON memberships (org);

CREATE TABLE devices (
    id            text PRIMARY KEY,
    org           text NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    user_id       text NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    machine       text NOT NULL,
    name          text NOT NULL,
    signed_in     bigint NOT NULL,
    last_seen     bigint NOT NULL,
    offline_until bigint,
    CONSTRAINT devices_user_machine UNIQUE (user_id, machine)
);
CREATE INDEX devices_org ON devices (org, last_seen DESC);

-- Secrets by SHA-256 digest only. Deleting a computer deletes its access and refresh tokens.
CREATE TABLE tokens (
    kind         text NOT NULL,
    digest       text NOT NULL,
    user_id      text NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device       text REFERENCES devices (id) ON DELETE CASCADE,
    invite_org   text REFERENCES organisations (id) ON DELETE CASCADE,
    invite_role  text,
    invite_email text,
    expires      bigint NOT NULL,
    PRIMARY KEY (kind, digest),
    CHECK ((invite_org IS NULL) = (invite_role IS NULL) AND (invite_org IS NULL) = (invite_email IS NULL))
);
CREATE INDEX tokens_user ON tokens (user_id);
CREATE INDEX tokens_device ON tokens (device);
CREATE INDEX tokens_expires ON tokens (expires);

-- Refresh tokens already swapped: a second use means the token was copied.
CREATE TABLE retired_refresh (
    digest  text PRIMARY KEY,
    device  text NOT NULL REFERENCES devices (id) ON DELETE CASCADE,
    expires bigint NOT NULL
);
CREATE INDEX retired_refresh_device ON retired_refresh (device);
CREATE INDEX retired_refresh_expires ON retired_refresh (expires);

CREATE TABLE device_codes (
    digest        text PRIMARY KEY,
    user_code     text NOT NULL CONSTRAINT device_codes_user_code_key UNIQUE,
    machine       text NOT NULL,
    device_name   text NOT NULL,
    -- pending / approved / denied / collected; approved_user only with approved.
    state         text NOT NULL,
    approved_user text,
    interval_secs bigint NOT NULL,
    last_poll     bigint NOT NULL,
    expires       bigint NOT NULL
);
CREATE INDEX device_codes_expires ON device_codes (expires);

CREATE TABLE surveys (
    id               text PRIMARY KEY,
    org              text NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    client_survey_id text NOT NULL,
    revision         bigint NOT NULL,
    project          text NOT NULL,
    line             text,
    schema_id        text NOT NULL,
    state            text NOT NULL,
    -- No foreign key: the survey stays when the person who uploaded it leaves.
    uploaded_by      text NOT NULL,
    uploaded         bigint NOT NULL,
    committed        bigint,
    assets           bigint NOT NULL,
    photos           bigint NOT NULL,
    photo_bytes      bigint NOT NULL,
    severity_counts  bigint[] NOT NULL,
    CONSTRAINT surveys_revision UNIQUE (org, client_survey_id, revision)
);
CREATE INDEX surveys_org_uploaded ON surveys (org, uploaded DESC);

-- The photos a survey's manifest lists.
CREATE TABLE survey_photos (
    survey text NOT NULL REFERENCES surveys (id) ON DELETE CASCADE,
    sha    text NOT NULL,
    bytes  bigint NOT NULL,
    kind   text NOT NULL,
    PRIMARY KEY (survey, sha)
);

-- The photos an organisation stores (shared by its surveys).
CREATE TABLE photos (
    org   text NOT NULL REFERENCES organisations (id) ON DELETE CASCADE,
    sha   text NOT NULL,
    bytes bigint NOT NULL,
    PRIMARY KEY (org, sha)
);
