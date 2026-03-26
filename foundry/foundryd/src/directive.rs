use foundry_core::types::IssuePhase;
use serde::{Deserialize, Serialize};

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
    #[allow(dead_code)]
    pub gitea_url: String,
    #[allow(dead_code)]
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

pub fn build_directive(ctx: &DirectiveContext) -> String {
    match ctx.phase {
        IssuePhase::Planning => format!(
            "You are a software developer assistant. Your task:\n\
             1. Read issue #{issue} in repository {owner}/{repo}: \"{title}\"\n\
             2. Analyze the issue carefully\n\
             3. Post a comment on the issue asking clarifying questions or proposing an implementation plan\n\
             4. Do NOT write any code yet — only communicate via comments\n\
             5. Wait for human feedback before proceeding",
            issue = ctx.issue_number,
            owner = ctx.owner,
            repo = ctx.repo,
            title = ctx.issue_title,
        ),
        IssuePhase::Implementing => {
            let branch = ctx
                .branch_name
                .clone()
                .unwrap_or_else(|| format!("foundry/issue-{}", ctx.issue_number));
            format!(
                "You are a software developer assistant. Your task:\n\
                 1. Clone the repository {owner}/{repo}\n\
                 2. Create branch `{branch}` from the default branch\n\
                 3. Implement the solution for issue #{issue}: \"{title}\"\n\
                 4. Push the branch to the remote\n\
                 5. Open a pull request referencing issue #{issue}\n\
                 6. Write a result.json file to /foundry/result.json with the format: {{\"pr_number\": N}}\n\
                 7. Exit when complete",
                owner = ctx.owner,
                repo = ctx.repo,
                branch = branch,
                issue = ctx.issue_number,
                title = ctx.issue_title,
            )
        }
        IssuePhase::InReview => {
            let pr = ctx.pr_number.unwrap_or(0);
            let summary = ctx
                .pending_event_summary
                .as_deref()
                .unwrap_or("New review feedback is available");
            format!(
                "You are a software developer assistant. Your task:\n\
                 1. Check out the branch for PR #{pr} in {owner}/{repo}\n\
                 2. Read the review comments and feedback\n\
                 3. Address all review feedback by updating the code\n\
                 4. Push your changes to the existing branch — do NOT force-push\n\
                 5. Summary of pending events: {summary}",
                pr = pr,
                owner = ctx.owner,
                repo = ctx.repo,
                summary = summary,
            )
        }
        IssuePhase::Done => "Exit immediately — this issue is done.".to_string(),
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
        ("pending_event_summary", ctx.pending_event_summary.as_deref().unwrap_or("")),
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let directive = build_directive(&ctx);
        assert!(directive.contains("issue #42"), "Should mention issue #42");
        assert!(directive.contains("alice/myproject"), "Should mention alice/myproject");
        assert!(!directive.contains("write") || directive.contains("Do NOT write"), "Should not instruct to write code");
    }

    #[test]
    fn implementing_mentions_branch_and_result_json() {
        let ctx = implementing_ctx();
        let directive = build_directive(&ctx);
        assert!(directive.contains("foundry/issue-7"), "Should mention branch name");
        assert!(directive.contains("result.json"), "Should mention result.json");
    }

    #[test]
    fn in_review_mentions_pr_and_no_force_push() {
        let ctx = in_review_ctx();
        let directive = build_directive(&ctx);
        assert!(directive.contains("PR #11") || directive.contains("#11"), "Should mention PR #11");
        assert!(directive.contains("force-push") || directive.contains("force push"), "Should warn about force-push");
    }

    #[test]
    fn instruction_serializes_with_correct_phase_string() {
        let ctx = planning_ctx();
        let instruction = build_instruction(&ctx);
        assert_eq!(instruction.phase, "planning");
        let json = serde_json::to_string(&instruction).unwrap();
        assert!(json.contains("\"phase\":\"planning\""));
        assert!(json.contains("\"issue_number\":42"));
    }

    #[test]
    fn done_phase_directive_is_exit() {
        let mut ctx = planning_ctx();
        ctx.phase = IssuePhase::Done;
        let directive = build_directive(&ctx);
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
}
