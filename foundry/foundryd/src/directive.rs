use crate::config::PhasePromptConfig;
use foundry_core::types::IssuePhase;
use serde::{Deserialize, Serialize};

const PLANNING_DEFAULT_PROMPT: &str =
    "You are a software developer assistant. Your task:\n\
     1. Read issue #{{issue_number}} in repository {{owner}}/{{repo}}: \"{{issue_title}}\"\n\
     2. Analyze the issue carefully\n\
     3. Post a comment on the issue asking clarifying questions or proposing an implementation plan\n\
     4. Do NOT write any code yet — only communicate via comments\n\
     5. Wait for human feedback before proceeding
     6. Always post a comment at the end of your turn to update the user with your status.";

const IMPLEMENTING_DEFAULT_PROMPT: &str =
    "You are a software developer assistant. Your task:\n\
    1. Comment on #{{issue_number}} in repository {{owner}}/{{repo}} to acknowledge the user's approval\n\
    2. Clone the repository {{owner}}/{{repo}}\n\
    3. Create branch `{{branch_name}}` from the default branch\n\
    4. Read all comments for issue #{{issue_number}}: \"{{issue_title}}\"\n\
    5. Implement the plan outlined in the issue comments\n\
    6. Dispatch an independent subagent to review the implementation for code quality and errors. Fix any findings.\n\
    7. Push the branch to the remote\n\
    8. Open a pull request referencing issue #{{issue_number}}\n\
    9. Add a comment to the issue referencing the pull request\n\
    10. Write a result.json file to /foundry/result.json with the format: {\"pr_number\": N}\n\
    11. Exit when complete";

const IN_REVIEW_DEFAULT_PROMPT: &str = "You are a software developer assistant. Your task:\n\
    1. Check out the branch for PR #{{pr_number}} in {{owner}}/{{repo}}\n\
    2. Read the review comments and feedback\n\
    3. Address all review feedback by updating the code\n\
    4. Push your changes to the existing branch — do NOT force-push\n\
    6. Add a reply to the review explaining your changes or asking clarifying questions\n\
    7. Summary of pending events: {{pending_event_summary}}";

const DONE_DEFAULT_PROMPT: &str = "Exit immediately — this issue is done.";

#[derive(Debug, Clone)]
pub struct DirectiveContext {
    pub phase: IssuePhase,
    pub owner: String,
    pub repo: String,
    pub issue_number: u64,
    pub issue_title: String,
    pub branch_name: Option<String>,
    pub pr_number: Option<u64>,
    pub pending_event_summary: Option<String>,
    pub gitea_url: String,
    pub bot_username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstructionRepo {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Instruction {
    pub phase: String,
    pub repo: InstructionRepo,
    pub issue_number: u64,
    pub pr_number: Option<u64>,
    pub directive: String,
}

pub fn build_default_prompt(ctx: &DirectiveContext) -> String {
    match ctx.phase {
        IssuePhase::Planning => render_template(PLANNING_DEFAULT_PROMPT, ctx),
        IssuePhase::Implementing => {
            let branch = ctx
                .branch_name
                .clone()
                .unwrap_or_else(|| format!("foundry/issue-{}", ctx.issue_number));
            let resolved = DirectiveContext {
                branch_name: Some(branch),
                ..ctx.clone()
            };
            render_template(IMPLEMENTING_DEFAULT_PROMPT, &resolved)
        }
        IssuePhase::InReview => {
            let summary = ctx
                .pending_event_summary
                .clone()
                .unwrap_or_else(|| "New review feedback is available".to_string());
            let resolved = DirectiveContext {
                pending_event_summary: Some(summary),
                ..ctx.clone()
            };
            render_template(IN_REVIEW_DEFAULT_PROMPT, &resolved)
        }
        IssuePhase::Done => DONE_DEFAULT_PROMPT.to_string(),
    }
}

pub fn render_template(template: &str, ctx: &DirectiveContext) -> String {
    let pr_number_str = ctx.pr_number.map(|n| n.to_string()).unwrap_or_default();
    let issue_number_str = ctx.issue_number.to_string();
    let vars: &[(&str, &str)] = &[
        ("owner", &ctx.owner),
        ("repo", &ctx.repo),
        ("issue_number", &issue_number_str),
        ("issue_title", &ctx.issue_title),
        ("gitea_url", &ctx.gitea_url),
        ("bot_username", &ctx.bot_username),
        ("branch_name", ctx.branch_name.as_deref().unwrap_or("")),
        ("pr_number", &pr_number_str),
        (
            "pending_event_summary",
            ctx.pending_event_summary.as_deref().unwrap_or(""),
        ),
    ];

    let mut result = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(open) = remaining.find("{{") {
        result.push_str(&remaining[..open]);
        let after_open = &remaining[open + 2..];
        if let Some(close) = after_open.find("}}") {
            let key = &after_open[..close];
            if let Some(&(_, value)) = vars.iter().find(|&&(k, _)| k == key) {
                result.push_str(value);
            } else {
                result.push_str("{{");
                result.push_str(key);
                result.push_str("}}");
            }
            remaining = &after_open[close + 2..];
        } else {
            // No closing }}, emit the rest as-is
            result.push_str("{{");
            remaining = after_open;
        }
    }
    result.push_str(remaining);
    result
}

pub fn build_phase_prompt(
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

pub fn build_instruction(
    ctx: &DirectiveContext,
    phase_cfg: Option<&PhasePromptConfig>,
) -> Instruction {
    Instruction {
        phase: ctx.phase.to_string(),
        repo: InstructionRepo {
            owner: ctx.owner.clone(),
            repo: ctx.repo.clone(),
        },
        issue_number: ctx.issue_number,
        pr_number: ctx.pr_number,
        directive: build_phase_prompt(build_default_prompt(ctx), phase_cfg, ctx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PhasePromptConfig;

    #[test]
    fn apply_phase_prompt_returns_base_when_no_config() {
        let ctx = planning_ctx();
        let result = build_phase_prompt("default".to_string(), None, &ctx);
        assert_eq!(result, "default");
    }

    #[test]
    fn apply_phase_prompt_replaces_base_when_prompt_set() {
        let cfg = PhasePromptConfig {
            prompt: Some("custom {{owner}}".to_string()),
            prompt_append: None,
        };
        let ctx = planning_ctx();
        let result = build_phase_prompt("default".to_string(), Some(&cfg), &ctx);
        assert_eq!(result, "custom alice");
    }

    #[test]
    fn apply_phase_prompt_appends_when_only_append_set() {
        let cfg = PhasePromptConfig {
            prompt: None,
            prompt_append: Some("extra {{repo}}".to_string()),
        };
        let ctx = planning_ctx();
        let result = build_phase_prompt("default".to_string(), Some(&cfg), &ctx);
        assert_eq!(result, "default\n\nextra myproject");
    }

    #[test]
    fn apply_phase_prompt_replaces_and_appends_when_both_set() {
        let cfg = PhasePromptConfig {
            prompt: Some("custom".to_string()),
            prompt_append: Some("appended".to_string()),
        };
        let ctx = planning_ctx();
        let result = build_phase_prompt("default".to_string(), Some(&cfg), &ctx);
        assert_eq!(result, "custom\n\nappended");
    }

    #[test]
    fn apply_phase_prompt_empty_string_prompt_falls_back_to_default() {
        let cfg = PhasePromptConfig {
            prompt: Some("".to_string()),
            prompt_append: Some("extra".to_string()),
        };
        let ctx = planning_ctx();
        let result = build_phase_prompt("default".to_string(), Some(&cfg), &ctx);
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
        let result = build_phase_prompt("default".to_string(), Some(&cfg), &ctx);
        assert_eq!(result, "default");
    }

    fn planning_ctx() -> DirectiveContext {
        DirectiveContext {
            phase: IssuePhase::Planning,
            owner: "alice".into(),
            repo: "myproject".into(),
            issue_number: 42,
            issue_title: "Fix the login bug".into(),
            branch_name: None,
            pr_number: None,
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        }
    }

    fn implementing_ctx() -> DirectiveContext {
        DirectiveContext {
            phase: IssuePhase::Implementing,
            owner: "alice".into(),
            repo: "myproject".into(),
            issue_number: 7,
            issue_title: "Add dark mode".into(),
            branch_name: None,
            pr_number: None,
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        }
    }

    fn in_review_ctx() -> DirectiveContext {
        DirectiveContext {
            phase: IssuePhase::InReview,
            owner: "alice".into(),
            repo: "myproject".into(),
            issue_number: 42,
            issue_title: "Fix the login bug".into(),
            branch_name: None,
            pr_number: Some(11),
            pending_event_summary: None,
            gitea_url: "http://gitea.local".into(),
            bot_username: "foundry-bot".into(),
        }
    }

    #[test]
    fn planning_mentions_issue_and_repo() {
        let ctx = planning_ctx();
        let directive = build_default_prompt(&ctx);
        assert!(directive.contains("issue #42"), "Should mention issue #42");
        assert!(
            directive.contains("alice/myproject"),
            "Should mention alice/myproject"
        );
        assert!(
            !directive.contains("write") || directive.contains("Do NOT write"),
            "Should not instruct to write code"
        );
    }

    #[test]
    fn implementing_mentions_branch_and_result_json() {
        let ctx = implementing_ctx();
        let directive = build_default_prompt(&ctx);
        assert!(
            directive.contains("foundry/issue-7"),
            "Should mention branch name"
        );
        assert!(
            directive.contains("result.json"),
            "Should mention result.json"
        );
    }

    #[test]
    fn in_review_mentions_pr_and_no_force_push() {
        let ctx = in_review_ctx();
        let directive = build_default_prompt(&ctx);
        assert!(
            directive.contains("PR #11") || directive.contains("#11"),
            "Should mention PR #11"
        );
        assert!(
            directive.contains("force-push") || directive.contains("force push"),
            "Should warn about force-push"
        );
    }

    #[test]
    fn instruction_serializes_with_correct_phase_string() {
        let ctx = planning_ctx();
        let instruction = build_instruction(&ctx, None);
        assert_eq!(instruction.phase, "planning");
        let json = serde_json::to_string(&instruction).unwrap();
        assert!(json.contains("\"phase\":\"planning\""));
        assert!(json.contains("\"issue_number\":42"));
    }

    #[test]
    fn done_phase_directive_is_exit() {
        let mut ctx = planning_ctx();
        ctx.phase = IssuePhase::Done;
        let directive = build_default_prompt(&ctx);
        assert!(directive.contains("Exit"), "Done phase should say to exit");
    }

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
        let result = render_template(
            "b={{branch_name}} p={{pr_number}} s={{pending_event_summary}}",
            &ctx,
        );
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
}
