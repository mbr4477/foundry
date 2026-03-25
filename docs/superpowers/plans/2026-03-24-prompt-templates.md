# Prompt Template Overrides Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow users to override the per-phase Claude prompts in `foundry.toml` using `{{variable}}` templates, with optional full replacement and/or append per phase.

**Architecture:** Add `PromptsConfig` / `PhasePromptConfig` structs to `config.rs`, add `render_template` + `apply_phase_prompt` helpers to `directive.rs`, update `build_instruction` to accept an optional `PhasePromptConfig`, and wire `dispatcher::spawn_turn` to pass the correct phase config. A capturing `MockRuntime` variant enables an end-to-end dispatcher integration test.

**Tech Stack:** Rust, serde/toml (already used), tokio (already used), cargo test

---

## File Map

| Action | Path | Responsibility |
|--------|------|----------------|
| Modify | `foundry/foundryd/src/config.rs` | Add `PhasePromptConfig` and `PromptsConfig` structs |
| Modify | `foundry/foundryd/src/directive.rs` | Add `render_template`, `apply_phase_prompt`; update `build_instruction` signature |
| Modify | `foundry/foundryd/src/dispatcher.rs` | Update `spawn_turn` call site; extend `MockRuntime` for integration test |
| Modify | `foundry.toml.example` | Add commented-out `[prompts]` example block |

---

### Task 1: Config structs for prompt overrides

**Files:**
- Modify: `foundry/foundryd/src/config.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `foundry/foundryd/src/config.rs`:

```rust
#[test]
fn prompts_config_parses_when_present() {
    let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "s"
[gitea]
url = "http://g"
url_from_runner = "http://g"
api_token = "t"
bot_username = "bot"
bot_display_name = "Bot"
bot_email = "bot@local"
[container]
image = "img"
runtime = "docker"
network = "net"
memory_limit_mb = 512
cpu_limit = 1.0
max_concurrent = 1
timeout_secs = 60
[volumes]
issue_prefix = "p"
shared_volume = "s"
home_volume = "h"
[commands]
approve = "/approve"
[prompts.planning]
prompt = "custom planning"
prompt_append = "extra"
[prompts.implementing]
prompt_append = "impl extra"
"#;
    let config = Config::from_toml(toml).unwrap();
    let planning = config.prompts.planning.as_ref().unwrap();
    assert_eq!(planning.prompt.as_deref(), Some("custom planning"));
    assert_eq!(planning.prompt_append.as_deref(), Some("extra"));
    let implementing = config.prompts.implementing.as_ref().unwrap();
    assert!(implementing.prompt.is_none());
    assert_eq!(implementing.prompt_append.as_deref(), Some("impl extra"));
    assert!(config.prompts.in_review.is_none());
}

#[test]
fn prompts_config_defaults_when_absent() {
    let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "s"
[gitea]
url = "http://g"
url_from_runner = "http://g"
api_token = "t"
bot_username = "bot"
bot_display_name = "Bot"
bot_email = "bot@local"
[container]
image = "img"
runtime = "docker"
network = "net"
memory_limit_mb = 512
cpu_limit = 1.0
max_concurrent = 1
timeout_secs = 60
[volumes]
issue_prefix = "p"
shared_volume = "s"
home_volume = "h"
[commands]
approve = "/approve"
"#;
    let config = Config::from_toml(toml).unwrap();
    assert!(config.prompts.planning.is_none());
    assert!(config.prompts.implementing.is_none());
    assert!(config.prompts.in_review.is_none());
}

#[test]
fn prompts_config_ignores_unknown_phase_keys() {
    let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "s"
[gitea]
url = "http://g"
url_from_runner = "http://g"
api_token = "t"
bot_username = "bot"
bot_display_name = "Bot"
bot_email = "bot@local"
[container]
image = "img"
runtime = "docker"
network = "net"
memory_limit_mb = 512
cpu_limit = 1.0
max_concurrent = 1
timeout_secs = 60
[volumes]
issue_prefix = "p"
shared_volume = "s"
home_volume = "h"
[commands]
approve = "/approve"
[prompts.done]
prompt = "should be ignored"
"#;
    let result = Config::from_toml(toml);
    assert!(result.is_ok(), "Unknown prompt phase key should be silently ignored");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd config::tests 2>&1 | tail -20
```

Expected: compile error — `Config` has no `prompts` field.

- [ ] **Step 3: Add the structs and field to `config.rs`**

Add these two structs before the `Config` struct definition:

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

Add a field to the `Config` struct (after the `logging` field):

```rust
#[serde(default)]
pub prompts: PromptsConfig,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd config::tests 2>&1 | tail -20
```

Expected: all config tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/matthew/claudejr/foundry && git add foundryd/src/config.rs && git commit -m "feat: add PhasePromptConfig and PromptsConfig to config"
```

---

### Task 2: `render_template` function

**Files:**
- Modify: `foundry/foundryd/src/directive.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `foundry/foundryd/src/directive.rs`:

```rust
#[test]
fn render_template_substitutes_all_variables() {
    let ctx = DirectiveContext {
        phase: IssuePhase::InReview,
        owner: "alice".into(),
        repo: "myproject".into(),
        issue_number: 42,
        issue_title: "Fix the bug".into(),
        branch_name: Some("foundry/issue-42".into()),
        pr_number: Some(11),
        pending_event_summary: Some("review by bob".into()),
        gitea_url: "http://gitea.local".into(),
        bot_username: "foundry-bot".into(),
    };
    let template = "{{owner}}/{{repo}} #{{issue_number}} \"{{issue_title}}\" \
                    branch={{branch_name}} pr={{pr_number}} \
                    summary={{pending_event_summary}} \
                    url={{gitea_url}} bot={{bot_username}}";
    let result = render_template(template, &ctx);
    assert_eq!(
        result,
        "alice/myproject #42 \"Fix the bug\" branch=foundry/issue-42 \
         pr=11 summary=review by bob url=http://gitea.local bot=foundry-bot"
    );
}

#[test]
fn render_template_unknown_placeholder_passes_through() {
    let ctx = planning_ctx();
    let result = render_template("hello {{unknown_var}} world", &ctx);
    assert_eq!(result, "hello {{unknown_var}} world");
}

#[test]
fn render_template_option_none_renders_empty_string() {
    let ctx = DirectiveContext {
        phase: IssuePhase::Planning,
        owner: "alice".into(),
        repo: "repo".into(),
        issue_number: 1,
        issue_title: "title".into(),
        branch_name: None,
        pr_number: None,
        pending_event_summary: None,
        gitea_url: "http://g".into(),
        bot_username: "bot".into(),
    };
    let result = render_template("b={{branch_name}} p={{pr_number}} s={{pending_event_summary}}", &ctx);
    assert_eq!(result, "b= p= s=");
}

#[test]
fn render_template_does_not_reprocess_substituted_values() {
    // A substituted value containing {{...}} should not be further processed
    let ctx = DirectiveContext {
        phase: IssuePhase::Planning,
        owner: "{{repo}}".into(), // owner value looks like a placeholder
        repo: "myrepo".into(),
        issue_number: 1,
        issue_title: "t".into(),
        branch_name: None,
        pr_number: None,
        pending_event_summary: None,
        gitea_url: "http://g".into(),
        bot_username: "bot".into(),
    };
    let result = render_template("{{owner}}/{{repo}}", &ctx);
    // owner renders as "{{repo}}", but that is NOT re-substituted
    assert_eq!(result, "{{repo}}/myrepo");
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd directive::tests::render_template 2>&1 | tail -20
```

Expected: compile error — `render_template` not defined.

- [ ] **Step 3: Implement `render_template`**

Add this function to `foundry/foundryd/src/directive.rs` (after `build_directive`, before `build_instruction`):

```rust
pub fn render_template(template: &str, ctx: &DirectiveContext) -> String {
    template
        .replace("{{owner}}", &ctx.owner)
        .replace("{{repo}}", &ctx.repo)
        .replace("{{issue_number}}", &ctx.issue_number.to_string())
        .replace("{{issue_title}}", &ctx.issue_title)
        .replace("{{gitea_url}}", &ctx.gitea_url)
        .replace("{{bot_username}}", &ctx.bot_username)
        .replace("{{branch_name}}", ctx.branch_name.as_deref().unwrap_or(""))
        .replace("{{pr_number}}", &ctx.pr_number.map(|n| n.to_string()).unwrap_or_default())
        .replace(
            "{{pending_event_summary}}",
            ctx.pending_event_summary.as_deref().unwrap_or(""),
        )
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd directive::tests::render_template 2>&1 | tail -20
```

Expected: 4 new tests pass.

- [ ] **Step 5: Commit**

```bash
cd /Users/matthew/claudejr/foundry && git add foundryd/src/directive.rs && git commit -m "feat: add render_template to directive"
```

---

### Task 3: `apply_phase_prompt` function

**Files:**
- Modify: `foundry/foundryd/src/directive.rs`

- [ ] **Step 1: Add import for `PhasePromptConfig`**

At the top of `foundry/foundryd/src/directive.rs`, add:

```rust
use crate::config::PhasePromptConfig;
```

- [ ] **Step 2: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block:

```rust
use crate::config::PhasePromptConfig;

#[test]
fn apply_phase_prompt_returns_base_when_no_config() {
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), None, &ctx);
    assert_eq!(result, "default");
}

#[test]
fn apply_phase_prompt_replaces_base_when_prompt_set() {
    let cfg = PhasePromptConfig {
        prompt: Some("custom {{owner}}".to_string()),
        prompt_append: None,
    };
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), Some(&cfg), &ctx);
    assert_eq!(result, "custom alice");
}

#[test]
fn apply_phase_prompt_appends_when_only_append_set() {
    let cfg = PhasePromptConfig {
        prompt: None,
        prompt_append: Some("extra {{repo}}".to_string()),
    };
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), Some(&cfg), &ctx);
    assert_eq!(result, "default\n\nextra myproject");
}

#[test]
fn apply_phase_prompt_replaces_and_appends_when_both_set() {
    let cfg = PhasePromptConfig {
        prompt: Some("custom".to_string()),
        prompt_append: Some("appended".to_string()),
    };
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), Some(&cfg), &ctx);
    assert_eq!(result, "custom\n\nappended");
}

#[test]
fn apply_phase_prompt_empty_string_prompt_falls_back_to_default() {
    let cfg = PhasePromptConfig {
        prompt: Some("".to_string()),
        prompt_append: Some("extra".to_string()),
    };
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), Some(&cfg), &ctx);
    // empty prompt → use default base; then append
    assert_eq!(result, "default\n\nextra");
}

#[test]
fn apply_phase_prompt_empty_append_is_ignored() {
    let cfg = PhasePromptConfig {
        prompt: None,
        prompt_append: Some("".to_string()),
    };
    let ctx = planning_ctx();
    let result = apply_phase_prompt("default".to_string(), Some(&cfg), &ctx);
    assert_eq!(result, "default");
}
```

- [ ] **Step 3: Run tests to verify they fail**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd directive::tests::apply_phase_prompt 2>&1 | tail -20
```

Expected: compile error — `apply_phase_prompt` not defined.

- [ ] **Step 4: Implement `apply_phase_prompt`**

Add this function to `foundry/foundryd/src/directive.rs` (after `render_template`, before `build_instruction`):

```rust
pub fn apply_phase_prompt(
    base: String,
    phase_cfg: Option<&PhasePromptConfig>,
    ctx: &DirectiveContext,
) -> String {
    let Some(cfg) = phase_cfg else {
        return base;
    };
    let base = match cfg.prompt.as_deref().filter(|s| !s.is_empty()) {
        Some(template) => render_template(template, ctx),
        None => base,
    };
    match cfg.prompt_append.as_deref().filter(|s| !s.is_empty()) {
        Some(template) => format!("{}\n\n{}", base, render_template(template, ctx)),
        None => base,
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd directive::tests::apply_phase_prompt 2>&1 | tail -20
```

Expected: 6 new tests pass.

- [ ] **Step 6: Commit**

```bash
cd /Users/matthew/claudejr/foundry && git add foundryd/src/directive.rs && git commit -m "feat: add apply_phase_prompt to directive"
```

---

### Task 4: Update `build_instruction` and wire dispatcher

**Files:**
- Modify: `foundry/foundryd/src/directive.rs` (signature + one existing test)
- Modify: `foundry/foundryd/src/dispatcher.rs` (call site in `spawn_turn`)

- [ ] **Step 1: Update `build_instruction` in `directive.rs`**

Replace the existing `build_instruction` function:

```rust
// Before:
pub fn build_instruction(ctx: &DirectiveContext) -> Instruction {
    Instruction {
        phase: ctx.phase.to_string(),
        repo: InstructionRepo {
            owner: ctx.owner.clone(),
            repo: ctx.repo.clone(),
        },
        issue_number: ctx.issue_number,
        pr_number: ctx.pr_number,
        directive: build_directive(ctx),
    }
}
```

```rust
// After:
pub fn build_instruction(ctx: &DirectiveContext, phase_cfg: Option<&PhasePromptConfig>) -> Instruction {
    Instruction {
        phase: ctx.phase.to_string(),
        repo: InstructionRepo {
            owner: ctx.owner.clone(),
            repo: ctx.repo.clone(),
        },
        issue_number: ctx.issue_number,
        pr_number: ctx.pr_number,
        directive: apply_phase_prompt(build_directive(ctx), phase_cfg, ctx),
    }
}
```

- [ ] **Step 2: Fix the broken existing test in `directive.rs`**

In the `instruction_serializes_with_correct_phase_string` test, update the call from:

```rust
let instruction = build_instruction(&ctx);
```

to:

```rust
let instruction = build_instruction(&ctx, None);
```

- [ ] **Step 3: Update the call site in `dispatcher.rs`**

In `spawn_turn` in `foundry/foundryd/src/dispatcher.rs`, locate the line:

```rust
let instruction = build_instruction(&ctx);
```

Replace with:

```rust
let phase_cfg = match session.phase {
    IssuePhase::Planning     => self.config.prompts.planning.as_ref(),
    IssuePhase::Implementing => self.config.prompts.implementing.as_ref(),
    IssuePhase::InReview     => self.config.prompts.in_review.as_ref(),
    IssuePhase::Done         => None,
};
let instruction = build_instruction(&ctx, phase_cfg);
```

The existing import at the top of the file (`use crate::directive::{build_instruction, DirectiveContext}`) is already correct — no import change needed.

- [ ] **Step 4: Run all tests to verify they pass**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd 2>&1 | tail -30
```

Expected: all tests pass with no compile errors.

- [ ] **Step 5: Commit**

```bash
cd /Users/matthew/claudejr/foundry && git add foundryd/src/directive.rs foundryd/src/dispatcher.rs && git commit -m "feat: wire prompt template overrides through build_instruction and dispatcher"
```

---

### Task 5: Dispatcher integration test with capturing mock

**Files:**
- Modify: `foundry/foundryd/src/dispatcher.rs` (test section only)

- [ ] **Step 1: Extend `MockRuntime` to capture written bytes**

In the `#[cfg(test)] mod tests` block in `dispatcher.rs`, add `HashMap` to the existing `use std::sync::atomic` import line:

```rust
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
```

The current `MockRuntime` struct has one field (`spawn_count`). Replace the entire struct definition and its `new()` impl to add `written_volumes`:

```rust
struct MockRuntime {
    spawn_count: Arc<AtomicUsize>,
    written_volumes: Arc<Mutex<HashMap<(String, String), Vec<u8>>>>,
}

impl MockRuntime {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            spawn_count: Arc::new(AtomicUsize::new(0)),
            written_volumes: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}
```

Update the `write_to_volume` implementation in the `ContainerRuntime` impl block (replace the current no-op body):

```rust
async fn write_to_volume(
    &self,
    vol: &str,
    path: &str,
    contents: &[u8],
) -> Result<(), ContainerError> {
    let mut map = self.written_volumes.lock().await;
    map.insert((vol.to_string(), path.to_string()), contents.to_vec());
    Ok(())
}
```

- [ ] **Step 2: Write the failing integration test**

Add to the test module (after the existing tests):

```rust
#[tokio::test]
async fn planning_prompt_override_is_applied_to_instruction() {
    let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "secret"
[gitea]
url = "http://gitea.local"
url_from_runner = "http://host.docker.internal"
api_token = "token"
bot_username = "foundry-bot"
bot_display_name = "Foundry Bot"
bot_email = "bot@local"
[container]
image = "foundry-runner:latest"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 512
cpu_limit = 0.5
max_concurrent = 4
timeout_secs = 60
[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"
home_volume = "foundry-home"
[commands]
approve = "/approve"
[prompts.planning]
prompt = "CUSTOM PLANNING for {{owner}}/{{repo}} issue #{{issue_number}}"
"#;
    let config = Arc::new(Config::from_toml(toml).unwrap());
    let store: Arc<dyn SessionStore> = Arc::new(MemorySessionStore::new());
    let runtime = MockRuntime::new();
    let written = runtime.written_volumes.clone();
    let dispatcher = Dispatcher::new(store, runtime, config);

    dispatcher
        .handle_event(Event::IssueAssigned {
            repo: RepoId {
                owner: "alice".into(),
                repo: "proj".into(),
            },
            issue_number: 99,
            assigner: "bob".into(),
            delivery_id: "del-override".into(),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let map = written.lock().await;
    let vol_key = (
        "foundry-issue__alice__proj__99".to_string(),
        "instruction.json".to_string(),
    );
    let bytes = map.get(&vol_key).expect("instruction.json should have been written");
    let instruction: crate::directive::Instruction =
        serde_json::from_slice(bytes).expect("instruction.json should deserialize");
    assert!(
        instruction.directive.contains("CUSTOM PLANNING for alice/proj issue #99"),
        "directive should contain custom prompt, got: {}",
        instruction.directive
    );
}
```

- [ ] **Step 3: Run all tests to verify everything passes**

```bash
cd /Users/matthew/claudejr/foundry && cargo test -p foundryd 2>&1 | tail -30
```

Expected: all tests pass including the new integration test. (Note: the 50ms sleep is a timing dependency that works in normal environments; if it flakes in CI, increase to 100ms.)

- [ ] **Step 4: Commit**

```bash
cd /Users/matthew/claudejr/foundry && git add foundryd/src/dispatcher.rs && git commit -m "test: add dispatcher integration test for prompt template override"
```

---

### Task 6: Update `foundry.toml.example`

**Files:**
- Modify: `foundry.toml.example`

- [ ] **Step 1: Add the commented prompts section**

Append to the end of `/Users/matthew/claudejr/foundry.toml.example`:

```toml

# Optional: override the default prompts for each workflow stage.
# Use {{variable}} placeholders — see docs/superpowers/specs/2026-03-24-prompt-templates-design.md
# for the full variable reference.
# prompt replaces the entire directive; prompt_append appends to it (joined by \n\n).
# Omit either key to use the default behaviour.

# [prompts.planning]
# # Uncomment and edit to override the default planning prompt.
# # prompt replaces the entire directive; prompt_append appends to it (separated by \n\n).
# prompt = """
# You are a software developer assistant. Your task:
# 1. Read issue #{{issue_number}} in repository {{owner}}/{{repo}}: "{{issue_title}}"
# 2. Analyze the issue carefully
# 3. Post a comment on the issue asking clarifying questions or proposing an implementation plan
# 4. Do NOT write any code yet — only communicate via comments
# 5. Wait for human feedback before proceeding\
# """
# prompt_append = ""
#
# [prompts.implementing]
# # Uncomment and edit to override the default implementing prompt.
# # prompt replaces the entire directive; prompt_append appends to it (separated by \n\n).
# prompt = """
# You are a software developer assistant. Your task:
# 1. Clone the repository {{owner}}/{{repo}}
# 2. Create branch `{{branch_name}}` from the default branch
# 3. Implement the solution for issue #{{issue_number}}: "{{issue_title}}"
# 4. Push the branch to the remote
# 5. Open a pull request referencing issue #{{issue_number}}
# 6. Write a result.json file to /foundry/result.json with the format: {"pr_number": N}
# 7. Exit when complete\
# """
# prompt_append = ""
#
# [prompts.in_review]
# # Uncomment and edit to override the default in-review prompt.
# # prompt replaces the entire directive; prompt_append appends to it (separated by \n\n).
# prompt = """
# You are a software developer assistant. Your task:
# 1. Check out the branch for PR #{{pr_number}} in {{owner}}/{{repo}}
# 2. Read the review comments and feedback
# 3. Address all review feedback by updating the code
# 4. Push your changes to the existing branch — do NOT force-push
# 5. Summary of pending events: {{pending_event_summary}}\
# """
# prompt_append = ""
```

- [ ] **Step 2: Commit**

```bash
cd /Users/matthew/claudejr && git add foundry.toml.example && git commit -m "docs: add commented prompt template examples to foundry.toml.example"
```
