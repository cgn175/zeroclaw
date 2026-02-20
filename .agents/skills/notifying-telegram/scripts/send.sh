#!/usr/bin/env bash
#
# Send a message to Telegram
#
# Usage: ./send.sh "Your message here"
#
# Environment variables:
#   TELEGRAM_BOT_TOKEN - Bot token from @BotFather (required)
#   TELEGRAM_CHAT_ID   - Chat ID to send message to (required)

set -euo pipefail

MESSAGE="${1:-}"

if [[ -z "$MESSAGE" ]]; then
    echo "Usage: $0 \"message\"" >&2
    exit 1
fi

if [[ -z "${TELEGRAM_BOT_TOKEN:-}" ]]; then
    echo "Error: TELEGRAM_BOT_TOKEN environment variable not set" >&2
    exit 1
fi

if [[ -z "${TELEGRAM_CHAT_ID:-}" ]]; then
    echo "Error: TELEGRAM_CHAT_ID environment variable not set" >&2
    exit 1
fi

API_URL="https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}/sendMessage"

# Send message via Telegram Bot API
RESPONSE=$(curl -s -X POST "$API_URL" \
    -H "Content-Type: application/json" \
    -d "$(jq -n --arg chat_id "$TELEGRAM_CHAT_ID" --arg text "$MESSAGE" \
        '{chat_id: $chat_id, text: $text, parse_mode: "HTML"}')")

# Check if successful
OK=$(echo "$RESPONSE" | jq -r '.ok')
if [[ "$OK" != "true" ]]; then
    ERROR=$(echo "$RESPONSE" | jq -r '.description // "Unknown error"')
    echo "Error sending message: $ERROR" >&2
    exit 1
fi

MESSAGE_ID=$(echo "$RESPONSE" | jq -r '.result.message_id')
echo "Message sent (ID: $MESSAGE_ID)"
