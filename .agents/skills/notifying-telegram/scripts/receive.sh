#!/usr/bin/env bash
#
# Wait for a reply message from Telegram
#
# Usage: ./receive.sh [timeout_seconds]
#
# Environment variables:
#   TELEGRAM_BOT_TOKEN - Bot token from @BotFather (required)
#   TELEGRAM_CHAT_ID   - Chat ID to receive messages from (required)
#
# Returns: The text of the first new message received
# Exit code 1 on timeout or error

set -euo pipefail

TIMEOUT="${1:-300}"  # Default 5 minutes

if [[ -z "${TELEGRAM_BOT_TOKEN:-}" ]]; then
    echo "Error: TELEGRAM_BOT_TOKEN environment variable not set" >&2
    exit 1
fi

if [[ -z "${TELEGRAM_CHAT_ID:-}" ]]; then
    echo "Error: TELEGRAM_CHAT_ID environment variable not set" >&2
    exit 1
fi

API_URL="https://api.telegram.org/bot${TELEGRAM_BOT_TOKEN}"

# Get the latest update_id to know where to start
RESPONSE=$(curl -s "${API_URL}/getUpdates?limit=1&offset=-1")
LAST_UPDATE_ID=$(echo "$RESPONSE" | jq -r '.result[-1].update_id // 0')

# Start polling from the next update
OFFSET=$((LAST_UPDATE_ID + 1))
START_TIME=$(date +%s)

echo "Waiting for reply (timeout: ${TIMEOUT}s)..." >&2

while true; do
    CURRENT_TIME=$(date +%s)
    ELAPSED=$((CURRENT_TIME - START_TIME))
    
    if [[ $ELAPSED -ge $TIMEOUT ]]; then
        echo "Timeout waiting for reply" >&2
        exit 1
    fi
    
    REMAINING=$((TIMEOUT - ELAPSED))
    # Use min of 30 seconds or remaining time for long polling
    POLL_TIMEOUT=$((REMAINING < 30 ? REMAINING : 30))
    
    RESPONSE=$(curl -s --get "${API_URL}/getUpdates" \
        --data-urlencode "offset=${OFFSET}" \
        --data-urlencode "timeout=${POLL_TIMEOUT}" \
        --data-urlencode 'allowed_updates=["message"]')
    
    OK=$(echo "$RESPONSE" | jq -r '.ok')
    if [[ "$OK" != "true" ]]; then
        ERROR=$(echo "$RESPONSE" | jq -r '.description // "Unknown error"')
        echo "Error polling updates: $ERROR" >&2
        exit 1
    fi
    
    # Check for messages from our chat
    MESSAGE=$(echo "$RESPONSE" | jq -r --arg chat_id "$TELEGRAM_CHAT_ID" '
        .result[] | 
        select(.message != null) |
        select(.message.chat.id == ($chat_id | tonumber)) |
        select(.message.text != null) |
        .message.text' | head -n 1)
    
    if [[ -n "$MESSAGE" ]]; then
        # Update offset to acknowledge the message
        NEW_UPDATE_ID=$(echo "$RESPONSE" | jq -r '.result[-1].update_id')
        curl -s "${API_URL}/getUpdates?offset=$((NEW_UPDATE_ID + 1))" > /dev/null
        
        echo "$MESSAGE"
        exit 0
    fi
    
    # Update offset for any updates received (even non-message ones)
    LATEST=$(echo "$RESPONSE" | jq -r '.result[-1].update_id // empty')
    if [[ -n "$LATEST" ]]; then
        OFFSET=$((LATEST + 1))
    fi
done
