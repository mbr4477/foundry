# Prompt Template Overrides Design

**Date:** 2026-03-24
**Status:** Draft

## Overview

Allow users to override the prompts sent to Claude Code for each workflow stage via optional configuration in `foundry.toml`. Overrides support both full replacement and append-only modes, with `{{variable}}` placeholders substituted from runtime context.

## Config Schema

A new optional `[prompts]` section in `foundry.toml` with per-phase subsections. Omitting the section or any subsection falls back to the hardcoded defaults.

Unknown phase keys (e.g. `[prompts.done]`) are silently ignored — `PromptsConfig` must **not** use `#[serde(deny_unknown_fields)]`.

The TOML key for the InReview phase is `in_review` (underscore), consistent with TOML/Rust serde convention. This is intentionally different from the kebab-case `"in-review"` string used in `instruction.json`'s `phase` field.

`${}` env-var syntax is **not** expanded in prompt fields. Only `{{variable}}` template substitution is performed. The existing `SecretValue::resolve` mechanism applies only to `webhook_secret` and `api_token`.

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

Long prompt strings may use TOML multiline syntax:

```toml
[prompts.planning]
prompt = """
You are a senior developer working on {{owner}}/{{repo}}.
Analyze issue #{{issue_number}} and propose an implementation plan.
"""
```

Both `prompt` and `prompt_append` may be set simultaneously. Resolution order:
1. If `prompt` is set (and non-empty), use its rendered value as the base directive; otherwise use the hardcoded default.
2. If `prompt_append` is set (and non-empty), append `\n\n` followed by its rendered value to the base directive.

An explicitly set empty string (`prompt = ""`) is treated the same as omitting the key — it falls back to the default behavior. For example, `prompt = ""` with `prompt_append = "extra"` appends `"extra"` to the hardcoded default.

## Template Variables

Variables are substituted using `{{variable}}` syntax. Unknown placeholders pass through unreplaced.

**Rendering rules by type:**
- `String` fields (`owner`, `repo`, `gitea_url`, `bot_username`, `issue_title`): substituted directly; always have a value.
- `u64` fields (`issue_number`): rendered as their decimal string representation via `.to_string()`.
- `branch_name` (`Option<String>`): always `Some` at runtime (set unconditionally to `key.branch_name()`), so it never renders as empty in practice. If `None` is passed by a future caller, it renders as empty string — same rule as other `Option<String>` fields.
- `Option<u64>` fields (`pr_number`): rendered as the decimal string if `Some`, empty string if `None`. `None` can occur in any phase where a PR has not yet been opened.
- `Option<String>` fields (`pending_event_summary`): rendered as the string if `Some`, empty string if `None`. `None` can occur in any phase, including InReview (e.g. polling-recovery spawns may not always supply a summary).

**Substitution order:** `render_template` performs a single linear pass — each `{{variable}}` placeholder is replaced once in order. Rendered values are not re-processed; a variable value that itself contains `{{...}}` syntax will not trigger further substitution.

> **Note on `{{issue_title}}`:** In the current implementation, `issue_title` is populated with a synthesised fallback (`"Issue #<N>"`) rather than the human-readable title fetched from the Gitea API. A separate change would be required to fetch the real issue title. Users should be aware that `{{issue_title}}` currently renders as e.g. `"Issue #42"`.

| Variable | Planning | Implementing | InReview | Description |
|---|---|---|---|---|
| `{{owner}}` | ✓ | ✓ | ✓ | Repository owner |
| `{{repo}}` | ✓ | ✓ | ✓ | Repository name |
| `{{issue_number}}` | ✓ | ✓ | ✓ | Issue number |
| `{{issue_title}}` | ✓ | ✓ | ✓ | Issue title (currently a synthesised fallback — see note above) |
| `{{gitea_url}}` | ✓ | ✓ | ✓ | Gitea instance URL (container-facing) |
| `{{bot_username}}` | ✓ | ✓ | ✓ | Bot account username |
| `{{branch_name}}` | ✓† | ✓ | ✓† | Derived branch name (†branch may not exist yet outside Implementing) |
| `{{pr_number}}` | ✓ | ✓ | ✓ | Pull request number (empty string when `None`) |
| `{{pending_event_summary}}` | ✓ | ✓ | ✓ | Pending review event summary (empty string when `None`) |

## Architecture

### Files Changed

**`foundryd/src/config.rs`**

Add two new structs:

```rust
#[derive(Debug, Deserialize, Default)]
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
- `apply_phase_prompt(base: String, phase_cfg: Option<&PhasePromptConfig>, ctx: &DirectiveContext) -> String` — takes ownership of `base` (the result of `build_directive` is passed by value). Applies the override logic: replaces base with rendered `prompt` if non-empty, then appends `\n\n` + rendered `prompt_append` if non-empty. When `phase_cfg` is `None`, returns `base` unchanged.

Update `build_instruction` to accept an additional `phase_cfg` parameter:

```rust
pub fn build_instruction(ctx: &DirectiveContext, phase_cfg: Option<&PhasePromptConfig>) -> Instruction
```

Internally it calls `build_directive(ctx)` to get the base, then passes it through `apply_phase_prompt`. `build_directive` remains unchanged.

**`foundryd/src/dispatcher.rs`**

**Breaking change:** the existing `build_instruction(&ctx)` call inside `spawn_turn` must be updated to pass `phase_cfg`. (Line number reflects codebase at spec-write time; reference by function name to avoid fragility.)

`spawn_turn` selects the correct `Option<&PhasePromptConfig>` from `self.config.prompts` based on the session phase:

```rust
let phase_cfg = match session.phase {
    IssuePhase::Planning     => self.config.prompts.planning.as_ref(),
    IssuePhase::Implementing => self.config.prompts.implementing.as_ref(),
    IssuePhase::InReview     => self.config.prompts.in_review.as_ref(),
    IssuePhase::Done         => None,
};
let instruction = build_instruction(&ctx, phase_cfg);
```

### Data Flow

```
foundry.toml
    └─ PromptsConfig
         └─ PhasePromptConfig (planning / implementing / in_review)
              │
              ▼
dispatcher::spawn_turn
    └─ build_instruction(ctx, phase_cfg)
         │  (ctx.issue_title is currently a synthesised fallback)
         ├─ build_directive(ctx)          → base string
         └─ apply_phase_prompt(base, phase_cfg, ctx)
              ├─ if phase_cfg is None: return base unchanged
              ├─ render_template(prompt, ctx)        → replaces base if non-empty
              └─ render_template(prompt_append, ctx) → appended with \n\n if non-empty
                   │
                   ▼
              Instruction.directive  →  instruction.json  →  claude -p
```

## Scope Exclusions

- The `Done` phase is excluded — it is dead code in the current dispatcher (sessions are deleted directly on PR merge/close; no container is ever spawned for Done).
- No per-repo prompt overrides — config is global.
- No conditional logic or loops in templates — variable substitution only.
- No `${}` env-var expansion in prompt fields.

## Testing

**`directive.rs` unit tests:**
- `render_template`: all variables substituted correctly; unknown variables pass through; `Option<u64>` renders as number or empty string; `Option<String>` renders as string or empty string.
- `apply_phase_prompt`: prompt only (replaces base); prompt_append only (appends to default with `\n\n`); both set (replaces then appends); neither set (returns default unchanged); `phase_cfg` is `None` (returns base unchanged); empty string `prompt` with non-empty `prompt_append` appends to the hardcoded default.

**`config.rs` unit tests:**
- `[prompts]` section parses correctly when fully specified.
- Omitting `[prompts]` entirely produces a default `PromptsConfig` with all fields `None`.
- A config containing `[prompts.done]` with valid-looking keys parses successfully via `Config::from_toml` with no error (unknown key silently ignored).

**`dispatcher.rs` integration test:**
- Construct a config with a `[prompts.planning]` override. `MockRuntime::write_to_volume` currently discards bytes — extend it to capture written bytes (e.g. store in an `Arc<Mutex<HashMap<String, Vec<u8>>>>`) before writing this test. Trigger `spawn_turn` for a Planning-phase session via `handle_event`. Assert by deserializing the captured bytes as `Instruction` and checking that `instruction.directive` equals or contains the expected overridden text (do not use raw byte substring matching, as JSON escaping may produce unexpected results).

**`foundry.toml` (repo root):**
- Add a commented-out `[prompts]` example block at the bottom of `/Users/matthew/claudejr/foundry.toml` demonstrating all three phases and all available variables:

```toml
# [prompts.planning]
# prompt = """
# You are a software developer assistant working on {{owner}}/{{repo}}.
# Your task:
# 1. Read issue #{{issue_number}}: "{{issue_title}}"
# 2. Analyze the issue and post a comment with clarifying questions or an implementation plan
# 3. Do NOT write any code yet
# """
# prompt_append = "Always respond in the same language as the issue."
#
# [prompts.implementing]
# prompt = """
# You are a software developer assistant. Your task:
# 1. Clone {{owner}}/{{repo}} and create branch {{branch_name}}
# 2. Implement the solution for issue #{{issue_number}}: "{{issue_title}}"
# 3. Push the branch, open a pull request, and write /foundry/result.json
# """
# prompt_append = ""
#
# [prompts.in_review]
# prompt = """
# You are a software developer assistant. Your task:
# 1. Check out the branch for PR #{{pr_number}} in {{owner}}/{{repo}}
# 2. Address all review feedback: {{pending_event_summary}}
# 3. Push your changes — do NOT force-push
# """
# prompt_append = ""
```
