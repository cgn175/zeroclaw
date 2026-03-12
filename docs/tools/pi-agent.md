# pi.dev Agent Integration

ZeroClaw features native sub-agent integration with the [pi.dev](https://pi.dev) terminal-based coding agent. This integration allows the primary ZeroClaw agent to intelligently delegate complex, multi-file software development, coding, and debugging tasks to a dedicated runtime agent explicitly built for codebase manipulation.

## Architecture

The `pi.dev` integration uses a **Tool-based Delegation Pattern** rather than acting as a raw LLM Provider. 

ZeroClaw registers a generic `PiAgentTool` (`pi_coding_agent`) which acts as a bridge. When the ZeroClaw LLM encounters a complex codebase task that is best handled by an autonomous coding specialist, it invokes the tool with a `task` description and a `cwd` (working directory).

The tool then securely spawns the `pi` CLI sub-process, captures the outcome, and hands the textual response back to ZeroClaw.

## Configuration & Setup

### Prerequisites

1. The `pi` CLI must be installed and available on the system `$PATH` where ZeroClaw is running.
2. The ZeroClaw agent runtime must be configured with a valid LLM provider and API key.

### Enabling the Tool

The `pi.dev` tool is **opt-in disabled** by default to prevent unexpected sub-agent spawning. You must explicitly enable it in your `config.toml` (or workspace configuration):

```toml
[pi_agent]
enabled = true
```

## Provider & Key Passthrough

ZeroClaw prioritizes using a single unified "brain". Thus, the `pi_coding_agent` tool automatically forwards your ZeroClaw provider preferences to the `pi.dev` sub-agent. 

When the tool executes, it does the following:
* **Generates a Temporary Project Configuration:** It creates a transient `.pi/settings.json` local to the tool's execution directory to strictly configure `defaultProvider` and `defaultModel` to match your ZeroClaw settings.
* **Injects Authentication:** ZeroClaw securely passes its primary API key to `pi` via the expected environment variable for the selected provider (e.g., `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`).
* **Maps API URLs:** For highly custom providers like OpenRouter and Ollama, ZeroClaw injects the corresponding environment configuration (`OPENAI_BASE_URL` or `OLLAMA_HOST`).

## Security & Sandboxing

The `pi_agent` tool enforces ZeroClaw's robust security architecture:

1. **Environment Isolation (CWE-200 Prevention):** The plugin completely clears the parent shell environment (`env_clear()`) before spawning `pi`. It selectively reinjects *only* safe system variables and the precise API key necessary, preventing accidental leakage of other secrets into the sub-agent context.
2. **Directory Sandboxing:** The optional `cwd` parameter is strictly validated and canonicalized against the ZeroClaw workspace root. Attempting to traverse directories via symlinks or `../../` outside the allowed space returns an explicit block error.
3. **Autonomy Restrictions:** The tool is explicitly blocked if the ZeroClaw agent is operating under `ReadOnly` autonomy, as `pi.dev` requires write privileges for codebase modification.
4. **Action Budgeting:** `pi` execution decrements your action budget to mitigate runaway LLM loop spending.
5. **Compute Safeguards:** The process receives a strict 5-minute timeout and a 1MB standard output truncation limit to prevent payload flooding.
