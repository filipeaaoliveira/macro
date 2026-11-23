//! The `plan_review` Step: the factory's checks of a plan, the plan
//! reviewer, the revise loop's last visit, and the `split` Step.

use std::collections::VecDeque;

use uuid::Uuid;

use super::*;
use crate::domain::fakes::FollowUpMade as MadeFollowUp;
use crate::domain::ports::NoChildTasks;

/// Ends each task with the next of its answers, keeping every task.
#[derive(Clone, Default)]
struct Reviewer {
    answers: Arc<Mutex<VecDeque<(&'static str, String)>>>,
    tasks: Arc<Mutex<Vec<AgentTask>>>,
}

impl Reviewer {
    fn answering(answers: Vec<(&'static str, String)>) -> Self {
        Self {
            answers: Arc::new(Mutex::new(answers.into())),
            tasks: Arc::default(),
        }
    }

    fn asked(&self) -> usize {
        self.tasks.lock().expect("tasks").len()
    }
}

impl StepAgent for Reviewer {
    type Err = FakeError;

    async fn answer(&self, task: AgentTask) -> Result<AgentAnswer, FakeError> {
        self.tasks.lock().expect("tasks").push(task);
        let (outcome, text) = self
            .answers
            .lock()
            .expect("answers")
            .pop_front()
            .expect("a scripted answer");
        Ok(AgentAnswer {
            outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
            text,
        })
    }

    async fn follow_up(&self, _turn: AgentTurn) -> Result<AgentAnswer, FakeError> {
        Err(FakeError)
    }

    async fn run_check(
        &self,
        _command: crate::domain::models::AgentCommand,
    ) -> Result<Option<agent_session::domain::session_command::SessionCommandOutput>, FakeError>
    {
        Ok(None)
    }

    async fn stop(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }

    async fn release(
        &self,
        _key: StepKey,
        _role: &Role,
        _owner: &MacroUserIdStr<'static>,
    ) -> Result<(), FakeError> {
        Ok(())
    }
}

static REPOSITORY: LazyLock<Repository> = LazyLock::new(|| Repository {
    url: "https://github.com/macro/app".to_string(),
    base_branch: "main".to_string(),
});

static PLAN_REVIEWER: LazyLock<Role> =
    LazyLock::new(|| Role::try_from("plan-reviewer".to_string()).expect("valid"));

const REQUEST: &str = "Clean up slugs.\n\n## Done when\n\n- Slugs collapse repeated dashes\n";

/// The plan entry's routes in the `code` Flow.
static ROUTES: LazyLock<Vec<Outcome>> = LazyLock::new(|| {
    ["approve", "revise", "blockers", "oversized"]
        .map(|name| Outcome::try_from(name.to_string()).expect("valid"))
        .to_vec()
});

/// A plan that fits, editing `path`.
fn plan_editing(path: &str) -> String {
    format!(
        "Collapse dashes in `{path}`.\n\n```json\n{{\"size\": \"fits\", \
         \"files\": [{{\"path\": \"{path}\", \"change\": \"edit\"}}], \
         \"doneWhen\": [{{\"item\": \"Slugs collapse repeated dashes\", \"work\": \"Step 1\"}}]}}\n```"
    )
}

/// A plan that splits the task in two.
const OVERSIZED: &str = r#"Too big for one run.

```json
{"size": "oversized", "why": "The API and the page merge apart",
 "children": [{"name": "Add the slug API", "request": "Add it.\n\n## Done when\n\n- It answers"},
              {"name": "Show slugs on the page", "request": "Show it.\n\n## Done when\n\n- It shows"}]}
```"#;

/// A Step of the Run's `entry` at `seq`, its `visit`, ended `outcome` with
/// `output`.
fn ended(seq: u16, entry: &str, visit: u16, outcome: &str, output: &str) -> Step {
    Step {
        seq,
        entry: EntryId::try_from(entry.to_string()).expect("valid"),
        visit,
        started_at: Utc::now(),
        role: None,
        agent: None,
        passed_over: Vec::new(),
        end: Some(StepEnd {
            outcome: Outcome::try_from(outcome.to_string()).expect("valid"),
            output: output.to_string(),
            ended_at: Utc::now(),
        }),
        gate: None,
        checks: Vec::new(),
        terminal: Default::default(),
    }
}

/// GitHub with `src/slug.rs` on the base branch.
fn github() -> FakeGithub {
    let github = FakeGithub::default();
    github.state().files.insert(
        ("main".to_string(), "src/slug.rs".to_string()),
        "fn slug() {}".to_string(),
    );
    github
}

/// The `step` of a Run on the task `task`, after `earlier`.
fn review_input<'a>(
    key: StepKey,
    earlier: &'a [Step],
    task: Option<Uuid>,
    routes: &'a [Outcome],
) -> StepInput<'a> {
    StepInput {
        key,
        role: Some(&PLAN_REVIEWER),
        request: REQUEST,
        task_id: task,
        repository: Some(&REPOSITORY),
        routes,
        max_visits: Some(3),
        ..input(earlier)
    }
}

/// Run `step` at the entry's `visit`, recorded in `repo`.
async fn run_visit(
    agent: &Reviewer,
    github: &FakeGithub,
    repo: &FakeRepo,
    name: &str,
    input: StepInput<'_>,
    visit: u16,
) -> Result<crate::domain::models::StepReport, BuiltInStepError> {
    let entry = if name == "plan_review" {
        "plan-review"
    } else {
        name
    };
    let entry = EntryId::try_from(entry.to_string()).expect("valid");
    repo.start_step(input.key, &entry, visit)
        .await
        .expect("start");
    let steps = BuiltInSteps::new(
        agent.clone(),
        PullRequestSteps::new(github.clone(), repo.clone(), FakeWatches::default()),
    )
    .with_child_tasks(repo.clone());
    steps.run(&step(name), input).await
}

fn key(seq: u16) -> StepKey {
    StepKey {
        run_id: RunId::generate(),
        seq,
    }
}

#[tokio::test]
async fn a_plan_that_edits_a_file_the_base_lacks_goes_back_without_the_reviewer() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", &plan_editing("src/slugs.rs"))];

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "blockers");
    assert!(
        report.output.contains("`src/slugs.rs` is not a file"),
        "{}",
        report.output
    );
    assert_eq!(reviewer.asked(), 0);
}

#[tokio::test]
async fn a_revised_plan_the_reviewer_passes_goes_to_the_gate() {
    let reviewer = Reviewer::answering(vec![(
        "pass",
        "Checked the files and the test.\n\n```json\n{\"blockers\": []}\n```".to_string(),
    )]);
    let repo = FakeRepo::default();
    // The first plan named a file the base lacks; the review sent it back,
    // and the planner revised it.
    let earlier = [
        ended(1, "plan", 1, "done", &plan_editing("src/slugs.rs")),
        ended(
            2,
            "plan-review",
            1,
            "blockers",
            "`src/slugs.rs` is not a file",
        ),
        ended(3, "plan", 2, "done", &plan_editing("src/slug.rs")),
    ];

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(4), &earlier, None, &ROUTES),
        2,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "reviewed");
    assert!(
        report.output.starts_with("The plan review passed"),
        "{}",
        report.output
    );
    assert!(report.output.contains("Checked the files and the test."));
    assert_eq!(reviewer.asked(), 1);
    let tasks = reviewer.tasks.lock().expect("tasks");
    assert_eq!(tasks[0].role.as_str(), "plan-reviewer");
    assert!(tasks[0].prompt.starts_with("# Review the plan"));
}

#[tokio::test]
async fn the_reviewer_s_blockers_go_back_to_the_planner() {
    let reviewer = Reviewer::answering(vec![(
        "blockers",
        "The plan tests nothing.\n\n```json\n{\"blockers\": [{\"problem\": \"No test shows dashes collapse\", \"why\": \"The Done-when item needs one\"}]}\n```".to_string(),
    )]);
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", &plan_editing("src/slug.rs"))];

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "blockers");
    assert!(
        report
            .output
            .contains("1. Blocker: No test shows dashes collapse: The Done-when item needs one"),
        "{}",
        report.output
    );
}

#[tokio::test]
async fn blockers_left_at_the_last_visit_go_to_the_gate_with_the_plan() {
    let reviewer = Reviewer::answering(vec![(
        "blockers",
        "```json\n{\"blockers\": [{\"problem\": \"Still no test\"}]}\n```".to_string(),
    )]);
    let repo = FakeRepo::default();
    let earlier = [ended(5, "plan", 3, "done", &plan_editing("src/slug.rs"))];

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(6), &earlier, None, &ROUTES),
        3,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "reviewed");
    assert!(
        report
            .output
            .starts_with("The plan review still finds 1 blockers at its last round"),
        "{}",
        report.output
    );
    assert!(report.output.contains("Still no test"));
}

#[tokio::test]
async fn an_unmapped_done_when_item_and_a_missing_size_block_are_blockers() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let unmapped = "Collapse.\n\n```json\n{\"size\": \"fits\", \"files\": [{\"path\": \"src/slug.rs\", \"change\": \"edit\"}]}\n```";
    let no_block = [ended(1, "plan", 1, "done", "Collapse dashes.")];
    let no_mapping = [ended(1, "plan", 1, "done", unmapped)];

    let missing = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &no_block, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");
    let unmapped = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &no_mapping, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(missing.outcome.as_str(), "blockers");
    assert!(missing.output.contains("the plan has no size block"));
    assert_eq!(unmapped.outcome.as_str(), "blockers");
    assert!(
        unmapped.output.contains("Done-when item 1 "),
        "{}",
        unmapped.output
    );
    assert_eq!(reviewer.asked(), 0);
}

#[tokio::test]
async fn an_oversized_plan_goes_to_split_only_where_its_entry_routes_it() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", OVERSIZED)];
    let without_split = [Outcome::try_from("blockers".to_string()).expect("valid")];

    let routed = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");
    let unrouted = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &without_split),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(routed.outcome.as_str(), "oversized");
    assert!(routed.output.contains("- Show slugs on the page"));
    assert_eq!(unrouted.outcome.as_str(), "blockers");
    assert_eq!(reviewer.asked(), 0);
}

#[tokio::test]
async fn split_files_each_child_task_once_linked_to_the_run_s_task() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let parent = Uuid::now_v7();
    let earlier = [
        ended(1, "plan", 1, "done", OVERSIZED),
        ended(2, "plan-review", 1, "oversized", "Too big"),
    ];
    let split = key(3);
    repo.task_names
        .lock()
        .expect("task names")
        .insert(parent, "Clean up slugs".to_string());

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "split",
        review_input(split, &earlier, Some(parent), &[]),
        1,
    )
    .await
    .expect("split");
    // A retried attempt files the same tasks, so no second one.
    let again = run_visit(
        &reviewer,
        &github(),
        &repo,
        "split",
        review_input(split, &earlier, Some(parent), &[]),
        1,
    )
    .await
    .expect("split");

    assert_eq!(report.outcome, Outcome::done());
    assert_eq!(again.output, report.output);
    let filed: Vec<MadeFollowUp> = repo.follow_ups.lock().expect("follow ups").clone();
    assert_eq!(
        filed
            .iter()
            .map(|made| made.task.name.as_str())
            .collect::<Vec<_>>(),
        ["Add the slug API", "Show slugs on the page"]
    );
    assert!(filed.iter().all(|made| made.of == Some(parent)));
    assert!(filed[0].task.markdown.contains(&parent.to_string()));
    assert!(
        filed[0]
            .task
            .markdown
            .contains("\"documentName\":\"Clean up slugs\""),
        "{}",
        filed[0].task.markdown
    );
    for made in &filed {
        assert!(
            report.output.contains(&made.id.to_string()),
            "{}",
            report.output
        );
    }
    assert_eq!(reviewer.asked(), 0);
}

#[tokio::test]
async fn split_where_no_task_is_filed_fails_saying_so() {
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", OVERSIZED)];
    let input = review_input(key(2), &earlier, None, &[]);
    let steps = BuiltInSteps::new(
        Reviewer::default(),
        PullRequestSteps::new(github(), repo.clone(), FakeWatches::default()),
    )
    .with_child_tasks(NoChildTasks);

    let report = steps.run(&step("split"), input).await.expect("ran");

    assert_eq!(report.outcome, Outcome::failed());
    assert!(report.output.contains("files no tasks"));
}

/// A team's Flow that gives its review two rounds puts the plan to its
/// Gate at the second, as the Run's own Flow bounds it.
#[tokio::test]
async fn the_run_s_own_flow_bounds_the_review_s_rounds() {
    let reviewer = Reviewer::answering(vec![(
        "blockers",
        "```json\n{\"blockers\": [{\"problem\": \"Still no test\"}]}\n```".to_string(),
    )]);
    let repo = FakeRepo::default();
    let earlier = [ended(3, "plan", 2, "done", &plan_editing("src/slug.rs"))];
    let input = StepInput {
        max_visits: Some(2),
        ..review_input(key(4), &earlier, None, &ROUTES)
    };

    let report = run_visit(&reviewer, &github(), &repo, "plan_review", input, 2)
        .await
        .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "reviewed");
    assert!(
        crate::domain::plan_review::blockers_left(&report.output),
        "{}",
        report.output
    );
}

/// A child task that failed to be filed is filed when the Step is tried
/// again, and the one filed before is not filed twice.
#[tokio::test]
async fn a_split_retried_after_a_failure_files_what_is_missing_once() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", OVERSIZED)];
    let split = key(2);
    // The first child is filed; the second fails, as an outage would.
    repo.failing
        .lock()
        .expect("failing")
        .insert("create_follow_up", (1, 1));

    let failed = run_visit(
        &reviewer,
        &github(),
        &repo,
        "split",
        review_input(split, &earlier, None, &[]),
        1,
    )
    .await;
    let retried = run_visit(
        &reviewer,
        &github(),
        &repo,
        "split",
        review_input(split, &earlier, None, &[]),
        1,
    )
    .await
    .expect("split");

    let Err(error) = failed else {
        panic!("the first attempt fails");
    };
    assert!(error.retryable(), "{error}");
    assert_eq!(retried.outcome, Outcome::done());
    let names: Vec<String> = repo
        .follow_ups
        .lock()
        .expect("follow ups")
        .iter()
        .map(|made| made.task.name.clone())
        .collect();
    assert_eq!(names, ["Add the slug API", "Show slugs on the page"]);
}

/// Files every child task under another id than it is asked for.
#[derive(Clone)]
struct ElsewhereTasks;

impl crate::domain::ports::ChildTasks for ElsewhereTasks {
    type Err = FakeError;

    async fn file_child(
        &self,
        _user_id: &MacroUserIdStr<'static>,
        _of: Option<Uuid>,
        _task: &crate::domain::ports::FollowUpTask,
        _task_id: Uuid,
    ) -> Result<Option<crate::domain::ports::FollowUpMade>, FakeError> {
        Ok(Some(crate::domain::ports::FollowUpMade::OtherId(
            Uuid::now_v7().to_string(),
        )))
    }

    async fn task_name(
        &self,
        _user_id: &MacroUserIdStr<'static>,
        _task_id: Uuid,
    ) -> Result<Option<String>, FakeError> {
        Ok(None)
    }
}

/// A child task made under another id stops the Step for good: a retry
/// would make another.
#[tokio::test]
async fn a_child_task_made_under_another_id_stops_the_split() {
    let repo = FakeRepo::default();
    let earlier = [ended(1, "plan", 1, "done", OVERSIZED)];
    let steps = BuiltInSteps::new(
        Reviewer::default(),
        PullRequestSteps::new(github(), repo.clone(), FakeWatches::default()),
    )
    .with_child_tasks(ElsewhereTasks);

    let ran = steps
        .run(&step("split"), review_input(key(2), &earlier, None, &[]))
        .await;

    let Err(error) = ran else {
        panic!("the split stops");
    };
    assert!(!error.retryable(), "{error}");
}

/// A size block holding a child request with a code fence reads whole,
/// and the plan is split.
#[tokio::test]
async fn a_child_request_with_a_code_fence_still_splits() {
    let reviewer = Reviewer::answering(Vec::new());
    let repo = FakeRepo::default();
    let fenced = "Too big.\n\n```json\n{\"size\": \"oversized\", \"why\": \"Two parts\", \"children\": [{\"name\": \"Add the API\", \"request\": \"Add it:\\n\\n```rust\\nfn slug() {}\\n```\\n\\n## Done when\\n\\n- It answers\"}, {\"name\": \"Show it\", \"request\": \"Show it.\"}]}\n```";
    let earlier = [ended(1, "plan", 1, "done", fenced)];

    let report = run_visit(
        &reviewer,
        &github(),
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "oversized", "{}", report.output);
}

/// A person's note on the plan Gate stays in the planner's prompt after
/// the review, its rounds started over, sends the plan back again.
#[test]
fn a_revise_note_stays_in_the_history_after_the_review_sends_the_plan_back() {
    let mut answered = ended(6, "plan-review", 3, "reviewed", "Blockers left");
    answered.gate = Some(Gate {
        kind: GateKind::Flow,
        question: "Approve this plan?".to_string(),
        answers: vec![
            Outcome::try_from("approve".to_string()).expect("valid"),
            Outcome::try_from("revise".to_string()).expect("valid"),
        ],
        opened_at: Utc::now(),
        answer: Some(GateAnswer {
            answer: Outcome::try_from("revise".to_string()).expect("valid"),
            note: Some("Keep the old slugs working".to_string()),
            user_id: Some(JULIA.clone()),
            rule: None,
            approved_control_files: None,
            reply_confidence: None,
            by_stand_in: false,
            agent_session_id: None,
            answered_at: Utc::now(),
        }),
        asked_back: None,
        stand_in: None,
        stand_in_declined: None,
    });
    let earlier = [
        ended(5, "plan", 3, "done", "Plan three"),
        answered,
        ended(7, "plan", 4, "done", "Plan four"),
        ended(8, "plan-review", 1, "blockers", "A new blocker"),
    ];

    let history = super::super::history(&earlier);

    assert!(history.contains("A new blocker"), "{history}");
    assert!(history.contains("Keep the old slugs working"), "{history}");
    assert!(!history.contains("Plan three"), "{history}");
}

/// A path with characters a URL reads is asked of GitHub encoded, and
/// found.
#[tokio::test]
async fn a_planned_path_is_read_from_github_encoded() {
    let reviewer = Reviewer::answering(vec![(
        "pass",
        "```json\n{\"blockers\": []}\n```".to_string(),
    )]);
    let repo = FakeRepo::default();
    let github = github();
    github.state().files.insert(
        ("main".to_string(), "app/%5Bslug%5D.tsx".to_string()),
        "export default 1".to_string(),
    );
    let earlier = [ended(1, "plan", 1, "done", &plan_editing("app/[slug].tsx"))];

    let report = run_visit(
        &reviewer,
        &github,
        &repo,
        "plan_review",
        review_input(key(2), &earlier, None, &ROUTES),
        1,
    )
    .await
    .expect("reviewed");

    assert_eq!(report.outcome.as_str(), "reviewed", "{}", report.output);
}
