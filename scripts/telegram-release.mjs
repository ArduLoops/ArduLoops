#!/usr/bin/env node
// Sends the release post to the Telegram group: release/telegram/post.jpg with post.md as
// the caption (see .cursor/rules/release-notes.mdc).
//
//   node scripts/telegram-release.mjs [--tag 2.7.0] [--dry-run] [--force]
//
// Needs TELEGRAM_BOT_TOKEN and TELEGRAM_CHAT_ID. Without them, or with --dry-run, it only prints
// the caption as Telegram will get it. With GITHUB_TOKEN it sends one post per version (see below).

import { execSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const CAPTION_MAX = 1024; // visible characters in a photo caption
const MESSAGE_MAX = 4096;

const arg = (name) => {
  const i = process.argv.indexOf(`--${name}`);
  return i > 0 ? process.argv[i + 1] : undefined;
};
const fail = (msg) => {
  console.error(`telegram-release: ${msg}`);
  process.exit(1);
};

const latestTag = () => {
  try {
    return execSync("git describe --tags --abbrev=0", { cwd: root, stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
  } catch {
    return "";
  }
};
const tag = (arg("tag") || latestTag()).replace(/^v/, "");
if (!tag) fail("no --tag and no git tag");

const dir = join(root, "release", "telegram");
const textPath = join(dir, "post.md");
const imagePath = join(dir, "post.jpg");
if (!existsSync(textPath)) fail("release/telegram/post.md is missing");
if (!existsSync(imagePath)) fail("release/telegram/post.jpg is missing");
const md = readFileSync(textPath, "utf8").replace(/\r\n/g, "\n").trim();
const version = md.match(/^#+ .*?(\d+\.\d+\.\d+)/m)?.[1];
if (version !== tag) fail(`release/telegram/post.md is for ${version ?? "no version"}, not ${tag}`);

// Markdown -> Telegram HTML: headings and **bold** to <b>, [text](url) to <a>, "- " to "• ".
const esc = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const inline = (s) =>
  esc(s)
    .replace(/\*\*(.+?)\*\*/g, "<b>$1</b>")
    .replace(/\[([^\]]+)\]\((https?:[^)\s]+)\)/g, '<a href="$2">$1</a>');
const body = md
  .split("\n")
  .map((line) => {
    const h = line.match(/^#+\s+(.*)$/);
    if (h) return `<b>${inline(h[1])}</b>`;
    const li = line.match(/^[-*]\s+(.*)$/);
    return li ? `• ${inline(li[1])}` : inline(line);
  })
  .join("\n");
const repo = process.env.GITHUB_REPOSITORY || "ArduLoops/ArduLoops";
const links = `⬇️ <a href="https://github.com/${repo}/releases/tag/${tag}">Завантажити ${tag}</a> · <a href="https://arduloops.web.app/">Сайт</a>`;
const full = `${body}\n\n${links}`;

// Telegram counts the text after HTML parsing, in UTF-16 code units.
const visible = (html) => html.replace(/<[^>]+>/g, "").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&").length;
let caption = full;
let followUp = null;
if (visible(full) > CAPTION_MAX) {
  // Too long for a caption: the photo gets the heading and the links, the text comes as a reply.
  caption = `${body.split("\n")[0]}\n\n${links}`;
  followUp = body;
  if (visible(followUp) > MESSAGE_MAX) fail("release/telegram/post.md is longer than one Telegram message");
}

console.log(caption);
if (followUp) console.log(`\n----- reply -----\n${followUp}`);
console.log(`\ntelegram-release: ${tag}, caption ${visible(caption)}/${CAPTION_MAX}${followUp ? " + reply" : ""}`);

const token = process.env.TELEGRAM_BOT_TOKEN;
const chatId = process.env.TELEGRAM_CHAT_ID;
if (process.argv.includes("--dry-run") || !token || !chatId) {
  console.log(`telegram-release: dry run${!token || !chatId ? " (no TELEGRAM_BOT_TOKEN / TELEGRAM_CHAT_ID)" : ""}, nothing sent`);
  process.exit(0);
}

// One post per version. Before it sends, the script creates the ref refs/telegram/<tag>. GitHub
// creates a ref only once, so a re-run (a job re-run pulls this job in again) sees 422 and stops.
// The ref is deleted if the send fails. --force sends again; an existing ref is then kept.
const ghToken = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
const lockRef = `telegram/${tag}`;
async function gh(method, path, body) {
  return fetch(`https://api.github.com/repos/${repo}/${path}`, {
    method,
    headers: { Authorization: `Bearer ${ghToken}`, Accept: "application/vnd.github+json", "Content-Type": "application/json" },
    body: body && JSON.stringify(body),
  });
}
let locked = false;
if (!ghToken) {
  if (process.env.GITHUB_ACTIONS) fail("no GITHUB_TOKEN, so a re-run could post twice");
  console.log("telegram-release: no GITHUB_TOKEN, the one-post check is skipped");
} else {
  const sha = execSync("git rev-parse HEAD", { cwd: root }).toString().trim();
  const res = await gh("POST", "git/refs", { ref: `refs/${lockRef}`, sha });
  if (res.status === 201) locked = true;
  else if (res.status === 422 && !process.argv.includes("--force")) {
    console.log(`telegram-release: ${tag} is already posted (refs/${lockRef}), nothing sent. Use --force to post again.`);
    process.exit(0);
  } else if (res.status !== 422) fail(`cannot create refs/${lockRef}: ${res.status} ${await res.text()}`);
}

async function tg(method, body) {
  const res = await fetch(`https://api.telegram.org/bot${token}/${method}`, { method: "POST", body });
  const json = await res.json().catch(() => ({}));
  if (!json.ok) {
    if (locked && method === "sendPhoto") await gh("DELETE", `git/refs/${lockRef}`);
    fail(`${method} failed: ${json.description || res.status}`);
  }
  return json.result;
}

const form = new FormData();
form.set("chat_id", chatId);
form.set("caption", caption);
form.set("parse_mode", "HTML");
form.set("photo", new Blob([readFileSync(imagePath)], { type: "image/jpeg" }), `arduloops-${tag}.jpg`);
const msg = await tg("sendPhoto", form);
if (followUp) {
  await tg(
    "sendMessage",
    new URLSearchParams({
      chat_id: chatId,
      text: followUp,
      parse_mode: "HTML",
      reply_parameters: JSON.stringify({ message_id: msg.message_id }),
      link_preview_options: JSON.stringify({ is_disabled: true }),
    }),
  );
}
console.log(`telegram-release: sent, message ${msg.message_id}`);
