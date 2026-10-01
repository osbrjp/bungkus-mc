#!/usr/bin/env bash
# Builds the Slack Block Kit payload for a release and prints it to stdout,
# in bungkus-cli's style (.github/scripts/slack-release.sh there) with mc's
# :bungkus: emoji. Inputs (env): NAME, TAG, URL, BODY (GitHub release notes
# markdown), MASCOT_URL (public image). The workflow pipes the output to the
# Slack webhook; run it locally to preview the JSON.
#
# Webhook messages cannot carry a copy-to-clipboard button (Block Kit buttons
# only open a URL or call an app), so the install command gets its own code
# block, and the buttons open the release and the install script.
set -euo pipefail

INSTALL='curl -fsSL https://raw.githubusercontent.com/osbrjp/bungkus-mc/main/install.sh | bash'

# GitHub markdown → Slack mrkdwn: drop images, "# " titles, quote boxes
# (release-drafter's notices), rules and its intro line,
# "## / ### Heading" → "*Heading*", "* / - item" → "• item", "**b**" → "*b*",
# "[text](url)" → "<url|text>", then squeeze blank lines.
notes=$(printf '%s\n' "$BODY" | sed -E \
  -e '/^<img /d' \
  -e '/^!\[/d' \
  -e '/^>/d' \
  -e '/^---+$/d' \
  -e '/^This release includes the following changes:/d' \
  -e '/^If you did not expect this/d' \
  -e '/^# /d' \
  -e 's/^#{2,3} (.*)$/*\1*/' \
  -e 's/^[*-] /• /' \
  -e 's/\*\*([^*]+)\*\*/*\1*/g' \
  -e 's/\[([^]]+)\]\(([^)]+)\)/<\2|\1>/g' | cat -s | sed -e '/./,$!d')
[ -n "$notes" ] || notes="_No notes for this release._"

# Section text is capped at 3000 characters by Slack.
if [ "${#notes}" -gt 2900 ]; then
  notes="${notes:0:2900}"$'\n…'
fi

puns=(
  "Your agents, bungkus'd to go."
  "Bungkus satu! One release, to go."
  "Fresh from the kitchen, wrapped in banana leaf."
  "Mission control, now extra pedas."
  "Wrap it up, ship it out. Bungkus!"
)
pun=${puns[RANDOM % ${#puns[@]}]}

jq -n \
  --arg name "$NAME" --arg tag "$TAG" --arg url "$URL" \
  --arg notes "$notes" --arg pun "$pun" --arg mascot "$MASCOT_URL" \
  --arg install "$INSTALL" '
{
  text: "\($name) \($tag) is out",
  blocks: [
    {
      type: "section",
      text: { type: "mrkdwn", text: "*\($name) \($tag) is out* :bungkus:\n_\($pun)_" },
      accessory: { type: "image", image_url: $mascot, alt_text: "bungkus mascot" }
    },
    { type: "divider" },
    { type: "section", text: { type: "mrkdwn", text: $notes } },
    { type: "divider" },
    {
      type: "rich_text",
      elements: [
        { type: "rich_text_section", elements: [ { type: "text", text: "Install or update:", style: { bold: true } } ] },
        { type: "rich_text_preformatted", elements: [ { type: "text", text: $install } ] }
      ]
    },
    {
      type: "actions",
      elements: [
        { type: "button", text: { type: "plain_text", text: "View release" }, url: $url, style: "primary" },
        { type: "button", text: { type: "plain_text", text: "Install script" }, url: "https://github.com/osbrjp/bungkus-mc/blob/main/install.sh" }
      ]
    },
    {
      type: "context",
      elements: [
        { type: "mrkdwn", text: "Already installed? `\($name) update`, or press `U` inside mc." }
      ]
    }
  ]
}'
