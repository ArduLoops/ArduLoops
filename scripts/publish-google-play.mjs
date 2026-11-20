#!/usr/bin/env node
// Uploads a signed Android App Bundle (.aab) to Google Play via the Android Publisher REST API v3.
//
//   node scripts/publish-google-play.mjs --aab dist-apk/ArduLoops_2.8.0_aarch64.aab [--version 2.8.0] [--track auto] [--dry-run]
//
// Needs GOOGLE_PLAY_SERVICE_ACCOUNT_JSON in the environment. With --track auto (the default), it
// targets "production" once unlocked on the account, or falls back to "alpha" (Closed testing).

import { createSign } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const PACKAGE = "com.iampm.arduloops";
const API = `https://androidpublisher.googleapis.com/androidpublisher/v3/applications/${PACKAGE}`;
const UPLOAD_API = `https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications/${PACKAGE}`;

const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : fallback;
};
const fail = (msg) => {
  console.error(`publish-google-play: ${msg}`);
  process.exit(1);
};

const aabPath = arg("aab");
if (!aabPath || !existsSync(aabPath)) fail(`missing or non-existent --aab (${aabPath ?? "none"})`);

const cargo = readFileSync(join(root, "src-tauri", "Cargo.toml"), "utf8");
const version = (arg("version") || cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1] || "").replace(/^v/, "");
if (!version) fail("could not determine app version");
const [maj, min, pat] = version.split(".").map((n) => parseInt(n, 10) || 0);
const expectedCode = maj * 1_000_000 + min * 1_000 + pat;

function plainNotes(mdPath, fallback) {
  if (!existsSync(mdPath)) return fallback;
  const raw = readFileSync(mdPath, "utf8").replace(/\r\n/g, "\n");
  const lines = raw
    .split("\n")
    .filter((l) => !/^---+$/.test(l.trim()) && !/github\.com\/ArduLoops|arduloops\.web\.app|t\.me\/ArduLoops|youtube\.com/i.test(l))
    .map((l) =>
      l
        .replace(/^#+\s*/, "")
        .replace(/\*\*(.+?)\*\*/g, "$1")
        .replace(/`([^`]+)`/g, "$1")
        .replace(/\[([^\]]+)\]\([^)]+\)/g, "$1")
        .replace(/^[-*]\s+/, "• ")
        .trim(),
    )
    .filter(Boolean);
  const text = lines.join("\n").slice(0, 490);
  return text || fallback;
}

const enNotes = plainNotes(join(root, ".release-notes.md"), `ArduLoops ${version}`);
const releaseNotes = [{ language: "en-US", text: enNotes }];

console.log(`publish-google-play: ${PACKAGE} ${version} (${expectedCode}), aab=${aabPath}`);

const rawKey = process.env.GOOGLE_PLAY_SERVICE_ACCOUNT_JSON;
if (process.argv.includes("--dry-run") || !rawKey) {
  console.log(`publish-google-play: dry run${!rawKey ? " (no GOOGLE_PLAY_SERVICE_ACCOUNT_JSON)" : ""}, nothing uploaded`);
  process.exit(0);
}

const sa = JSON.parse(rawKey);
const b64url = (obj) => Buffer.from(typeof obj === "string" ? obj : JSON.stringify(obj)).toString("base64url");

async function accessToken() {
  const now = Math.floor(Date.now() / 1000);
  const header = b64url({ alg: "RS256", typ: "JWT" });
  const claim = b64url({
    iss: sa.client_email,
    scope: "https://www.googleapis.com/auth/androidpublisher",
    aud: sa.token_uri || "https://oauth2.googleapis.com/token",
    iat: now,
    exp: now + 3600,
  });
  const sign = createSign("RSA-SHA256");
  sign.update(`${header}.${claim}`);
  const sig = sign.sign(sa.private_key, "base64url");
  const res = await fetch(sa.token_uri || "https://oauth2.googleapis.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer",
      assertion: `${header}.${claim}.${sig}`,
    }),
  });
  const json = await res.json();
  if (!res.ok || !json.access_token) fail(`OAuth token failed: ${JSON.stringify(json)}`);
  return json.access_token;
}

const token = await accessToken();
async function api(url, method = "GET", body = undefined, contentType = "application/json") {
  const headers = { Authorization: `Bearer ${token}` };
  if (body !== undefined) headers["Content-Type"] = contentType;
  const res = await fetch(url, {
    method,
    headers,
    body: body === undefined ? undefined : contentType === "application/json" ? JSON.stringify(body) : body,
  });
  const text = await res.text();
  let json = {};
  try {
    json = text ? JSON.parse(text) : {};
  } catch {
    json = { raw: text };
  }
  return { ok: res.ok, status: res.status, json };
}

// 1. Create an edit session
const editRes = await api(`${API}/edits`, "POST", {});
if (!editRes.ok) fail(`create edit failed (${editRes.status}): ${JSON.stringify(editRes.json)}`);
const editId = editRes.json.id;

// 2. Upload the .aab bundle (or reuse if versionCode already exists in the library)
const aabBytes = readFileSync(aabPath);
let versionCode = expectedCode;
const upRes = await api(`${UPLOAD_API}/edits/${editId}/bundles?uploadType=media`, "POST", aabBytes, "application/octet-stream");
if (upRes.ok && upRes.json.versionCode) {
  versionCode = upRes.json.versionCode;
  console.log(`publish-google-play: uploaded bundle versionCode=${versionCode}`);
} else if (JSON.stringify(upRes.json).includes("version code") || upRes.status === 403 || upRes.status === 409) {
  console.log(`publish-google-play: bundle ${expectedCode} already in library, reusing (${upRes.status})`);
} else {
  fail(`bundle upload failed (${upRes.status}): ${JSON.stringify(upRes.json)}`);
}

// 3. Assign the bundle to the highest available track (production -> alpha -> internal)
const requestedTrack = arg("track", "auto");
const tracks = requestedTrack === "auto" ? ["production", "alpha", "internal"] : [requestedTrack];

let assignedTrack = null;
let assignedStatus = null;
for (const track of tracks) {
  for (const status of ["completed", "draft"]) {
    const trRes = await api(`${API}/edits/${editId}/tracks/${track}`, "PUT", {
      track,
      releases: [
        {
          name: version,
          versionCodes: [String(versionCode)],
          status,
          releaseNotes,
        },
      ],
    });
    if (trRes.ok) {
      // Validate before committing so we know if this track/status is allowed on the account
      const valRes = await api(`${API}/edits/${editId}:validate`, "POST");
      if (valRes.ok) {
        assignedTrack = track;
        assignedStatus = status;
        break;
      }
      console.log(`publish-google-play: validate ${track} (${status}) -> ${valRes.json?.error?.message || valRes.status}`);
    } else {
      console.log(`publish-google-play: track ${track} (${status}) -> ${trRes.json?.error?.message || trRes.status}`);
    }
  }
  if (assignedTrack) break;
}

if (!assignedTrack) fail("could not validate release on any track");

// 4. Commit the edit
const commitRes = await api(`${API}/edits/${editId}:commit`, "POST");
if (!commitRes.ok) {
  // Retry commit with changesNotSentForReview=true if the app requires manual review submission in Play Console
  const retryRes = await api(`${API}/edits/${editId}:commit?changesNotSentForReview=true`, "POST");
  if (!retryRes.ok) fail(`commit failed (${commitRes.status}): ${JSON.stringify(commitRes.json)}`);
  console.log(`publish-google-play: committed ${version} (${versionCode}) to track '${assignedTrack}' (${assignedStatus}, changesNotSentForReview=true)`);
} else {
  console.log(`publish-google-play: committed ${version} (${versionCode}) to track '${assignedTrack}' (${assignedStatus})`);
}
