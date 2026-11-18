use super::*;

fn other_person() -> MacroUserIdStr<'static> {
    MacroUserIdStr::try_from_email("editor@example.com").expect("a valid user id")
}

/// Mentioning the agent in a thread, by its starter or anyone following up
/// there, follows the session for nobody; neither does a runtime forwarding a
/// triggered prompt nor a bot acting for nobody. A prompt a person types into
/// the session itself follows it for that person, owner or not.
#[tokio::test]
async fn only_prompts_typed_into_the_session_follow_it() {
    let ((service, repo, containers, _announcer, _runtimes), mut turns) =
        harness_with_signals(PromptContextMock::default(), PromptComposerMock::default());
    let id = AgentSessionId::new();
    let container = live_session(&service, &containers, id).await;
    let agent = container.agent();
    turns.settled(id).await;
    assert!(
        repo.followers(id).is_empty(),
        "the opening mention follows nothing"
    );

    let follow_up = service.execute(
        id,
        HarnessCommand::Deliver(forward_message("a follow-up in the thread")),
    );
    let (result, ()) = tokio::join!(follow_up, agent.completes_prompt());
    result.expect("a thread follow-up should deliver");
    turns.settled(id).await;

    for (actor, context) in [(Some(other_person()), Some(trigger())), (None, None)] {
        let prompted = service.control_event(
            id,
            ControlEvent {
                action: AgentAction::prompt("not typed by a person"),
                action_id: None,
                actor,
                context,
                client: None,
                prompter: agent_session::domain::model::PrompterKind::User,
            },
        );
        let (result, ()) = tokio::join!(prompted, agent.completes_prompt());
        result.expect("the prompt should deliver");
        turns.settled(id).await;
    }
    assert!(
        repo.followers(id).is_empty(),
        "thread, forwarded and bot prompts follow nothing"
    );

    for person in [other_person(), sender()] {
        let prompted = service.control_event(
            id,
            ControlEvent {
                action: AgentAction::prompt("typed into the session"),
                action_id: None,
                actor: Some(person),
                context: None,
                client: None,
                prompter: agent_session::domain::model::PrompterKind::User,
            },
        );
        let (result, ()) = tokio::join!(prompted, agent.completes_prompt());
        result.expect("a direct prompt should deliver");
        turns.settled(id).await;
    }
    assert_eq!(repo.followers(id), vec![other_person(), sender()]);
}

/// The tab a person typed a prompt in is recorded on the turn it opens, so
/// the turn can show them things there; the next turn's prompter replaces it.
#[tokio::test]
async fn a_typed_prompt_records_its_tab_on_the_turn() {
    let ((service, repo, containers, _announcer, _runtimes), mut turns) =
        harness_with_signals(PromptContextMock::default(), PromptComposerMock::default());
    let id = AgentSessionId::new();
    let container = live_session(&service, &containers, id).await;
    let agent = container.agent();
    turns.settled(id).await;
    let tab = agent_session::domain::model::UiClientId::from_uuid(Uuid::from_u128(0xc1));

    for client in [Some(tab), None] {
        let prompted = service.control_event(
            id,
            ControlEvent {
                action: AgentAction::prompt("open the plan"),
                action_id: None,
                actor: Some(sender()),
                context: None,
                client,
                prompter: agent_session::domain::model::PrompterKind::User,
            },
        );
        let (result, ()) = tokio::join!(prompted, agent.completes_prompt());
        result.expect("a direct prompt should deliver");
        turns.settled(id).await;

        let prompter = repo
            .turn_prompter(id)
            .await
            .expect("the prompter is readable")
            .expect("the turn recorded its prompter");
        assert_eq!(prompter.client, client);
    }
}

/// A prompt sent for its owner by work that runs out of sight, as a
/// software factory Step's are, follows the session for no one, so no list
/// shows it; the same person's typed prompt still follows it.
#[tokio::test]
async fn an_out_of_sight_prompt_follows_the_session_for_no_one() {
    let ((service, repo, containers, _announcer, _runtimes), mut turns) =
        harness_with_signals(PromptContextMock::default(), PromptComposerMock::default());
    let id = AgentSessionId::new();
    let container = live_session(&service, &containers, id).await;
    let agent = container.agent();
    turns.settled(id).await;

    let mut out_of_sight = AgentAction::prompt("go on with the step");
    if let AgentAction::Prompt(prompt) = &mut out_of_sight {
        prompt.set_out_of_sight();
    }
    let prompted = service.control_event(
        id,
        ControlEvent {
            action: out_of_sight,
            action_id: None,
            actor: Some(sender()),
            context: None,
            client: None,
        },
    );
    let (result, ()) = tokio::join!(prompted, agent.completes_prompt());
    result.expect("the prompt should deliver");
    turns.settled(id).await;
    assert!(
        repo.followers(id).is_empty(),
        "an out-of-sight prompt follows the session for no one"
    );

    let typed = service.control_event(
        id,
        ControlEvent {
            action: AgentAction::prompt("typed into the session"),
            action_id: None,
            actor: Some(sender()),
            context: None,
            client: None,
        },
    );
    let (result, ()) = tokio::join!(typed, agent.completes_prompt());
    result.expect("a direct prompt should deliver");
    turns.settled(id).await;
    assert_eq!(repo.followers(id), vec![sender()]);
}
