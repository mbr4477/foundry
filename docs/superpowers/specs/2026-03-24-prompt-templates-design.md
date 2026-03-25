# Prompt Template Overrides Design

**Date:** 2026-03-24
**Status:** Approved

## Overview

Allow users to override the prompts sent to Claude Code for each workflow stage via optional configuration in `foundry.toml`. Overrides support both full replacement and append-only modes, with `{{variable}}` placeholders substituted from runtime context.

## Config Schema

A new optional `[prompts]` section in `foundry.toml` with per-phase subsections. Omitting the section or any subsection falls back to the hardcoded defaults.

```toml
[prompts.planning]
prompt = "..."         # optional: fully replaces the default directive
prompt_append = "..."  # optional: appended to whichever directive is active

[prompts.implementing]
prompt = "..."
prompt_append = "..."

[prompts.in_review]
prompt = "..."
prompt_append = "..."
```

Both `prompt` and `prompt_append` may be set simultaneously. Resolution order:
1. If `prompt` is set, use its rendered value as the base directive; otherwise use the hardcoded default.
2. If `prompt_append` is set, append its rendered value to the base directive.

## Template Variables

Variables are substituted using `{{variable}}` syntax. Unknown placeholders pass through unreplaced. Variables whose value is `None` in context render as an empty string.

| Variable | Planning | Implementing | InReview | Description |
|---|---|---|---|---|
| `{{owner}}` | ✓ | ✓ | ✓ | Repository owner |
| `{{repo}}` | ✓ | ✓ | ✓ | Repository name |
| `{{issue_number}}` | ✓ | ✓ | ✓ | Issue number |
| `{{issue_title}}` | ✓ | ✓ | ✓ | Issue title |
| `{{gitea_url}}` | ✓ | ✓ | ✓ | Gitea instance URL |
| `{{bot_username}}` | ✓ | ✓ | ✓ | Bot account username |
| `{{branch_name}}` | — | ✓ | — | Feature branch name |
| `{{pr_number}}` | — | — | ✓ | Pull request number |
| `{{pending_event_summary}}` | — | — | ✓ | Summary of pending review events |

## Architecture

### Files Changed

**`foundryd/src/config.rs`**

Add two new structs:

```rust
#[derive(Debug, Deserialize)]
pub struct PhasePromptConfig {
    pub prompt: Option<String>,
    pub prompt_append: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct PromptsConfig {
    pub planning: Option<PhasePromptConfig>,
    pub implementing: Option<PhasePromptConfig>,
    pub in_review: Option<PhasePromptConfig>,
}
```

Add to `Config`:

```rust
#[serde(default)]
pub prompts: PromptsConfig,
```

**`foundryd/src/directive.rs`**

Add two functions:

- `render_template(template: &str, ctx: &DirectiveContext) -> String` — chains `.replace()` calls for each supported variable against the template string.
- `apply_phase_prompt(base: String, phase_cfg: Option<&PhasePromptConfig>, ctx: &DirectiveContext) -> String` — applies the override logic (replace base if `prompt` set, append if `prompt_append` set).

`build_directive` remains unchanged. `build_instruction` calls `apply_phase_prompt` after obtaining the base directive from `build_directive`, and accepts the relevant `Option<&PhasePromptConfig>` as an additional parameter.

**`foundryd/src/dispatcher.rs`**

`spawn_turn` extracts the correct `Option<&PhasePromptConfig>` from `self.config.prompts` based on the session phase and passes it to `build_instruction`.

### Data Flow

```
foundry.toml
    └─ PromptsConfig
         └─ PhasePromptConfig (planning / implementing / in_review)
              │
              ▼
dispatcher::spawn_turn
    └─ build_instruction(ctx, phase_cfg)
         ├─ build_directive(ctx)          → base string
         └─ apply_phase_prompt(base, phase_cfg, ctx)
              ├─ render_template(prompt, ctx)        → replaces base (if set)
              └─ render_template(prompt_append, ctx) → appended (if set)
                   │
                   ▼
              Instruction.directive  →  instruction.json  →  claude -p
```

## Scope Exclusions

- The `Done` phase is excluded — it is dead code in the current dispatcher (sessions are deleted directly on PR merge/close; no container is ever spawned).
- No per-repo prompt overrides — config is global.
- No conditional logic or loops in templates — variable substitution only.

## Testing

- Unit tests in `directive.rs` for `render_template` (all variables, unknown variables, None values) and `apply_phase_prompt` (prompt only, append only, both, neither).
- Update `config.rs` tests to verify `[prompts]` section parses correctly and that omitting it produces a default `PromptsConfig`.
- Existing directive tests remain valid — `build_directive` is unchanged.
