---
name: notifying-telegram
description: "Sends task completion notifications to Telegram and receives user responses. Use after completing tasks to notify user of results, ask questions, or get instructions."
---

# Telegram Notification Skill

Send notifications to Telegram after task completion and receive responses from the user.

## Configuration

Set these environment variables:
- `TELEGRAM_BOT_TOKEN`: Your Telegram bot token (from @BotFather)
- `TELEGRAM_CHAT_ID`: The chat ID to send/receive messages

## When to Use

Notify the user via Telegram:
- After completing a task (summary of what was done)
- When encountering an error or problem
- When you have a question and need user input
- When waiting for user decision before proceeding

## Workflow

### After Task Completion
1. Run `scripts/send.sh` with a short summary message
2. If you need a response, run `scripts/receive.sh` to wait for user reply
3. Parse the reply and continue accordingly

### Message Format Guidelines
- Keep messages concise (1-3 sentences)
- Use emojis for visual clarity:
  - ✅ Task completed successfully
  - ❌ Error occurred
  - ❓ Question / need input
  - ⏳ In progress / waiting
- Include relevant details (file names, error messages, options)

## Examples

### Notify task completion
```bash
./scripts/send.sh "✅ Refactored auth module. Changed 3 files, added 15 tests. All tests pass."
```

### Ask a question and wait for response
```bash
./scripts/send.sh "❓ Found 2 approaches for caching: Redis or in-memory. Which do you prefer?"
REPLY=$(./scripts/receive.sh 300)  # Wait up to 5 minutes
echo "User replied: $REPLY"
```

### Report an error
```bash
./scripts/send.sh "❌ Build failed: missing dependency 'lodash'. Should I add it to package.json?"
```

## Scripts

### send.sh
Sends a message to the configured Telegram chat.
```
Usage: ./scripts/send.sh "Your message here"
```

### receive.sh
Waits for a reply message from the user.
```
Usage: ./scripts/receive.sh [timeout_seconds]
Default timeout: 300 seconds (5 minutes)
```

Returns the user's reply text, or exits with code 1 if timeout.
