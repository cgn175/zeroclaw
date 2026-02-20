# A2A Deprecated Code Removal - Progress Report

**Date:** 2026-02-20  
**Status:** IN PROGRESS  
**Goal:** Remove all deprecated custom A2A protocol code, keep only Google A2A standard implementation

## Context

The Google A2A protocol migration was completed in commit `d11a14b`. However, deprecated/legacy code was kept for "backward compatibility during migration". This cleanup removes all deprecated code.

## Completed

### ✅ Protocol Layer (src/channels/a2a/protocol.rs)
- Removed deprecated `A2AMessage` struct (lines 656-702)
- Removed legacy message type documentation
- Kept: Google A2A types (AgentCard, Task, TaskMessage, Artifact, TaskUpdate)

### ✅ Channel Layer (src/channels/a2a/channel.rs)
- Removed deprecated `peer_send_url()` helper function
- Removed deprecated `peer_sse_url()` helper function  
- Removed A2AMessage import
- Kept: Google A2A implementation (peer_tasks_url, peer_task_stream_url)

### ✅ Module Exports (src/channels/a2a/mod.rs)
- Removed `pub use protocol::A2AMessage;` export
- Updated module documentation to reflect Google A2A protocol
- Kept: A2AChannel export

## Remaining Work

### ⚠️ Gateway Layer (src/gateway/a2a.rs)
**Status:** Needs cleanup  
**Issue:** File contains both old and new implementations

**Old code to remove (lines 1-797):**
- `POST /a2a/send` endpoint (`handle_a2a_send`)
- `GET /a2a/stream/:session_id` endpoint  
- `GET /a2a/health` endpoint
- `POST /a2a/pair/*` endpoints
- A2AMessage handling code
- Legacy rate limiter and idempotency store

**New code to keep (lines 798+):**
- `GET /.well-known/agent.json` (`handle_agent_card`)
- `POST /tasks` (`handle_create_task`)
- `GET /tasks/{id}` (`handle_get_task`)
- `GET /tasks/{id}/stream` (`handle_task_stream`)
- `POST /tasks/{id}/cancel` (`handle_cancel_task`)
- Task-based processing logic

### ⚠️ Processor Layer (src/channels/a2a/processor.rs)
**Status:** Needs review  
**Issue:** Still imports and uses A2AMessage

**Options:**
1. Remove entirely if Google A2A doesn't need it
2. Update to use Task instead of A2AMessage
3. Keep if it's used for internal message conversion

### ⚠️ Router Registration (src/gateway/mod.rs)
**Status:** Needs update  
**Action:** Ensure only Google A2A routes are registered

### ⚠️ CLI Commands (src/main.rs)
**Status:** Already removed in earlier cleanup
**Note:** A2A CLI commands were removed but may need verification

### ⚠️ Config Schema (src/config/schema.rs)
**Status:** Already cleaned
**Note:** A2A config structs were removed but may need verification

### ⚠️ Tests (tests/)
**Status:** Needs review  
**Action:** Update or remove tests that reference A2AMessage

### ⚠️ Documentation
**Status:** Needs update  
**Files to update:**
- `docs/channels-reference.md` - Update A2A section for Google protocol
- `docs/a2a-setup.md` - May need rewrite or removal
- `README.md` - Update A2A references if any

## Compilation Errors

Current errors after partial cleanup:

```
error[E0432]: unresolved import `super::protocol::A2AMessage`
error[E0432]: unresolved import `protocol::A2AMessage`  
error[E0599]: no function or associated item named `peer_sse_url` found
```

**Root cause:** A2AMessage was removed but still referenced in:
- `src/channels/a2a/processor.rs`
- `src/gateway/a2a.rs`
- `src/main.rs` (if not fully cleaned)
- `tests/a2a_integration.rs`

## Recommended Next Steps

1. **Complete gateway cleanup:**
   - Extract Google A2A endpoints to new clean file
   - Remove old `/a2a/*` endpoints
   - Update imports

2. **Fix processor:**
   - Update to use Task instead of A2AMessage
   - Or remove if not needed

3. **Verify compilation:**
   - `cargo check`
   - Fix remaining A2AMessage references

4. **Update tests:**
   - Rewrite for Google A2A protocol
   - Or remove deprecated tests

5. **Update documentation:**
   - Reflect Google A2A endpoints
   - Remove custom protocol docs

6. **Final verification:**
   - `cargo test`
   - `cargo clippy`
   - Manual testing with two agents

## Files Modified So Far

- `src/channels/a2a/protocol.rs` - Removed A2AMessage
- `src/channels/a2a/channel.rs` - Removed deprecated helpers
- `src/channels/a2a/mod.rs` - Removed A2AMessage export

## Files Still Needing Changes

- `src/gateway/a2a.rs` - Remove old endpoints (major cleanup)
- `src/channels/a2a/processor.rs` - Update or remove
- `src/gateway/mod.rs` - Update route registration
- `tests/a2a_integration.rs` - Update or remove
- `docs/channels-reference.md` - Update documentation
- `docs/a2a-setup.md` - Update or remove

## Decision Points

1. **Processor fate:** Keep and update, or remove entirely?
2. **Test strategy:** Rewrite for Google A2A or start fresh?
3. **Documentation:** Update existing or create new?

## Estimated Remaining Effort

- Gateway cleanup: 1-2 hours
- Processor update: 30 minutes
- Tests: 1 hour
- Documentation: 30 minutes
- **Total:** 3-4 hours
