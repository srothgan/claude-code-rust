// SPDX-License-Identifier: Apache-2.0

use super::*;
use crate::app::test_support::named_tool_call_info;

#[test]
fn terminal_statuses_remain_distinguishable_and_arbitrary_spinner_frames_are_safe() {
    let completed = status_icon(model::ToolCallStatus::Completed, crate::ui::SpinnerState::Static);
    assert!(!completed.0.is_empty());
    for status in [model::ToolCallStatus::Failed, model::ToolCallStatus::Killed] {
        let failure = status_icon(status, crate::ui::SpinnerState::Static);
        assert!(!failure.0.is_empty());
        assert_ne!(failure, completed, "failure must not look completed");
    }
    for frame in [0, crate::ui::SpinnerState::FRAMES.len(), 999_999] {
        assert!(
            !status_icon(
                model::ToolCallStatus::InProgress,
                crate::ui::SpinnerState::Animated(frame)
            )
            .0
            .is_empty()
        );
    }
}

struct TitleCase {
    tool: ToolCallInfo,
    visible: Vec<&'static str>,
    absent: Vec<&'static str>,
}

fn title_case(
    name: &str,
    title: &str,
    visible: &[&'static str],
    absent: &[&'static str],
) -> TitleCase {
    let mut tool = named_tool_call_info("title-target", name, model::ToolCallStatus::InProgress);
    tool.title = title.into();
    TitleCase { tool, visible: visible.to_vec(), absent: absent.to_vec() }
}

fn task_title_cases() -> Vec<TitleCase> {
    let mut cases = Vec::new();
    let mut background =
        title_case("Agent", "TASK_ORCHID", &["TASK_ORCHID", "backgrounded", "3"], &[]);
    background.tool.task_metadata =
        Some(model::TaskMetadata::new().backgrounded(Some(true)).spawn_depth(Some(3)));
    cases.push(background);
    let mut unrelated = title_case("Read", "READ_JASMINE", &["READ_JASMINE"], &["depth", "37"]);
    unrelated.tool.task_metadata = Some(model::TaskMetadata::new().spawn_depth(Some(37)));
    cases.push(unrelated);
    for name in ["Agent", "Task", "Bash", "Monitor", "Workflow"] {
        let mut restarted =
            title_case(name, "RESTART_TARGET", &["RESTART_TARGET", "worker restarted"], &[]);
        restarted.tool.status = model::ToolCallStatus::Killed;
        restarted.tool.task_metadata = Some(
            model::TaskMetadata::new()
                .terminal_status(Some("stopped".into()))
                .terminal_reason(Some("worker_restart".into())),
        );
        cases.push(restarted);
    }
    let mut skill =
        title_case("Skill", "Skill: FRONTEND_ORCHID", &["FRONTEND_ORCHID", "backgrounded"], &[]);
    skill.tool.status = model::ToolCallStatus::Completed;
    skill.tool.output_metadata = Some(
        model::ToolOutputMetadata::new()
            .skill(Some(model::SkillOutputMetadata::new().background(Some(true)))),
    );
    cases.push(skill);
    for (name, other) in [("Bash", "PowerShell"), ("PowerShell", "Bash")] {
        cases.push(title_case(name, "echo SHELL_ORCHID", &[name, "echo SHELL_ORCHID"], &[other]));
    }
    cases
}

fn agent_title_cases() -> Vec<TitleCase> {
    let mut cases = Vec::new();
    let mut requested = title_case(
        "Agent",
        "Agent: REVIEW_WORKER",
        &["REVIEW_WORKER", "REVIEW_KIND", "MODEL_REQUESTED"],
        &[],
    );
    requested.tool.raw_input = Some(
        serde_json::json!({"name":"REVIEW_WORKER", "subagent_type":"REVIEW_KIND", "model":"MODEL_REQUESTED"}),
    );
    cases.push(requested);
    let mut observed = title_case(
        "Agent",
        "Agent: REVIEW_WORKER",
        &["REVIEW_WORKER", "REVIEW_KIND", "MODEL_OBSERVED"],
        &["MODEL_REQUESTED"],
    );
    observed.tool.raw_input = Some(
        serde_json::json!({"name":"REVIEW_WORKER", "subagent_type":"REVIEW_KIND", "model":"MODEL_REQUESTED"}),
    );
    observed.tool.status = model::ToolCallStatus::Completed;
    observed.tool.output_metadata = Some(model::ToolOutputMetadata::new().agent(Some(
        model::AgentOutputMetadata::new().resolved_model(Some("MODEL_OBSERVED".into())),
    )));
    cases.push(observed);
    let mut resolved =
        title_case("Agent", "RESOLVED_ORCHID", &["RESOLVED_ORCHID", "MODEL_OBSERVED"], &[]);
    resolved.tool.status = model::ToolCallStatus::Completed;
    resolved.tool.output_metadata = Some(model::ToolOutputMetadata::new().agent(Some(
        model::AgentOutputMetadata::new().resolved_model(Some("MODEL_OBSERVED".into())),
    )));
    cases.push(resolved);
    let mut route = title_case(
        "Agent",
        "ROUTE_ORCHID",
        &["ROUTE_ORCHID", "MODEL_FIRST", "MODEL_LAST"],
        &["MODEL_REQUESTED"],
    );
    route.tool.status = model::ToolCallStatus::Completed;
    route.tool.raw_input = Some(serde_json::json!({"model":"MODEL_REQUESTED"}));
    route.tool.output_metadata = Some(
        model::ToolOutputMetadata::new().agent(Some(
            model::AgentOutputMetadata::new()
                .resolved_model(Some("MODEL_LAST".into()))
                .models_used(Some(vec!["MODEL_FIRST".into(), "MODEL_LAST".into()])),
        )),
    );
    cases.push(route);
    for (status, visible, absent) in [
        (model::ToolCallStatus::InProgress, &["RETRY_ORCHID", "retry", "2/4", "1.5s"][..], &[][..]),
        (model::ToolCallStatus::Completed, &["RETRY_ORCHID"][..], &["retry", "2/4", "1.5s"][..]),
    ] {
        let mut retry = title_case("Agent", "RETRY_ORCHID", visible, absent);
        retry.tool.status = status;
        retry.tool.task_metadata = Some(model::TaskMetadata::new().subagent_retry(Some(
            model::SubagentRetryUpdate::Waiting {
                agent_id: Some("agent-1".into()),
                attempt: 2,
                max_retries: 4,
                retry_delay_ms: 1_500,
                error_status: Some(429),
                error_category: Some("rate_limit".into()),
            },
        )));
        cases.push(retry);
    }
    cases
}

fn bash_title_cases() -> Vec<TitleCase> {
    [
        (
            model::BashOutputMetadata::new().assistant_auto_backgrounded(Some(true)),
            &["BASH_ORCHID", "assistant backgrounded"][..],
            &["10,000"][..],
        ),
        (
            model::BashOutputMetadata::new()
                .assistant_auto_backgrounded(Some(true))
                .timed_out_after_ms(Some(10_000)),
            &["BASH_ORCHID", "auto-backgrounded", "10,000"][..],
            &["assistant backgrounded"][..],
        ),
        (
            model::BashOutputMetadata::new().background_ends_with_final_response(Some(true)),
            &["BASH_ORCHID", "ends with final response"][..],
            &[][..],
        ),
    ]
    .into_iter()
    .map(|(metadata, visible, absent)| {
        let mut row = title_case("Bash", "BASH_ORCHID", visible, absent);
        row.tool.status = model::ToolCallStatus::Completed;
        row.tool.output_metadata = Some(model::ToolOutputMetadata::new().bash(Some(metadata)));
        row
    })
    .collect()
}

#[test]
fn tool_title_values_remain_associated_with_their_metadata_and_precedence() {
    for mut row in
        task_title_cases().into_iter().chain(agent_title_cases()).chain(bash_title_cases())
    {
        for frame in [0, 7] {
            let rendered = standard::render_tool_call_title(
                &row.tool,
                ToolCallRenderContext::default(),
                180,
                crate::ui::SpinnerState::Animated(frame),
            )
            .to_string();
            for value in &row.visible {
                assert!(rendered.contains(value), "missing {value}: {rendered}");
            }
            for value in &row.absent {
                assert!(!rendered.contains(value), "unexpected {value}: {rendered}");
            }
            if row.visible.contains(&"MODEL_FIRST") {
                assert!(
                    rendered.find("MODEL_FIRST").unwrap() < rendered.find("MODEL_LAST").unwrap(),
                    "observed route order is meaningful"
                );
            }
        }
        if row.visible.contains(&"retry") {
            row.tool.status = model::ToolCallStatus::Completed;
            let settled = standard::render_tool_call_title(
                &row.tool,
                ToolCallRenderContext::default(),
                180,
                crate::ui::SpinnerState::Static,
            )
            .to_string();
            assert!(settled.contains("RETRY_ORCHID"));
            assert!(
                !settled.contains("retry") && !settled.contains("2/4") && !settled.contains("1.5s"),
                "completed warmed title must discard transient retry values"
            );
        }
    }
}

#[test]
fn non_execution_feedback_and_staged_write_outcomes_stay_in_their_card_bodies() {
    let mut rejected = named_tool_call_info("rejected", "Bash", model::ToolCallStatus::Failed);
    rejected.title = "REJECTED_TARGET".into();
    rejected.output_metadata = Some(
        model::ToolOutputMetadata::new().non_execution(Some(
            model::ToolNonExecutionMetadata::new("user-rejected")
                .user_feedback(Some("Use the ORCHID read command.".into())),
        )),
    );
    let mut staged = named_tool_call_info("staged", "Write", model::ToolCallStatus::Completed);
    staged.title = "STAGED_TARGET".into();
    staged.output_metadata = Some(model::ToolOutputMetadata::new().staged(true));
    for (mut tool, title, required, absent) in [
        (
            rejected,
            "REJECTED_TARGET",
            &["rejected by the user", "Use the ORCHID read command."][..],
            &["file unchanged"][..],
        ),
        (
            staged,
            "STAGED_TARGET",
            &["Staged for review", "file unchanged"][..],
            &["Use the ORCHID read command."][..],
        ),
    ] {
        let mut rows = Vec::new();
        render_tool_call_cached(
            &mut tool,
            ToolCallRenderContext::default(),
            100,
            crate::ui::SpinnerState::Static,
            &mut rows,
        );
        assert!(rows[0].to_string().contains(title));
        let body = rows[1..].iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        for expected in required {
            assert!(body.contains(expected), "{body}");
        }
        for forbidden in absent {
            assert!(!body.contains(forbidden), "{body}");
        }
    }
}
