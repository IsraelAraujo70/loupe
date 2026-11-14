//! Reads from a real T3 server. Run with
//! `T3_URL=... T3_TOKEN_FILE=... T3_PR=owner/name#number \
//!  cargo test -p loupe-t3 --test live -- --ignored --nocapture`.
//! `T3_TOKEN_FILE` is a path to a file that holds the token; only titles,
//! counts, deep links, and provider and model names are printed.

use loupe_t3::{PullRequestKey, T3, thread_deep_link};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

fn parse_pr(text: &str) -> PullRequestKey {
    let (repository, number) = text
        .rsplit_once('#')
        .unwrap_or_else(|| panic!("T3_PR must look like owner/name#number"));
    let number = number.parse().expect("T3_PR number");
    PullRequestKey::github(repository, number)
}

#[test]
#[ignore = "needs a live T3 server: T3_URL, T3_TOKEN_FILE, T3_PR"]
fn live_linked_threads_and_deep_links() {
    let token = std::fs::read_to_string(var("T3_TOKEN_FILE")).expect("read T3_TOKEN_FILE");
    let t3 = T3::new(var("T3_URL"), token.trim());
    let pr = parse_pr(&var("T3_PR"));

    let environment = t3.environment().expect("environment");
    assert!(!environment.id.is_empty());
    println!(
        "environment {:?} (server {})",
        environment.label, environment.server_version
    );

    let threads = t3.linked_threads(&pr).expect("linked threads");
    assert!(
        !threads.is_empty(),
        "no thread is linked to the pull request"
    );
    for linked in &threads {
        println!(
            "linked {:?} in {:?}: {}{} -> {}",
            linked.title,
            linked.project,
            linked.status.label(),
            if linked.settled { ", settled" } else { "" },
            thread_deep_link(&environment.id, &linked.id)
        );
    }
}

/// Prints each provider instance, whether it can run a thread, and its
/// models with their efforts.
#[test]
#[ignore = "needs a live T3 server: T3_URL, T3_TOKEN_FILE"]
fn live_lists_the_providers_and_their_models() {
    let token = std::fs::read_to_string(var("T3_TOKEN_FILE")).expect("read T3_TOKEN_FILE");
    let t3 = T3::new(var("T3_URL"), token.trim());
    let providers = t3.providers().expect("providers");
    assert!(!providers.is_empty(), "the server reports no provider");
    for provider in &providers {
        println!(
            "{} ({}): {}",
            provider.name,
            provider.instance_id,
            provider.unavailable.as_deref().unwrap_or("ready")
        );
        for model in &provider.models {
            let efforts: Vec<String> = model
                .efforts
                .iter()
                .map(|effort| format!("{}{}", effort.id, if effort.is_default { "*" } else { "" }))
                .collect();
            println!("  {} [{}]: {}", model.slug, model.name, efforts.join(" "));
        }
    }
    assert!(
        providers
            .iter()
            .any(|provider| provider.unavailable.is_none() && !provider.models.is_empty()),
        "no provider can run a thread"
    );
}

#[test]
#[ignore = "needs a live T3 server: T3_URL"]
fn live_rejects_an_invalid_token() {
    let t3 = T3::new(var("T3_URL"), "loupe-deliberately-invalid-token");
    let environment = t3.environment().expect("environment");
    assert!(!environment.id.is_empty());
    println!(
        "environment {} {:?} (server {})",
        environment.id, environment.label, environment.server_version
    );
    let result = t3.linked_threads(&PullRequestKey::github("owner/name", 1));
    assert!(
        matches!(result, Err(loupe_t3::Error::Unauthorized)),
        "expected Unauthorized, got {result:?}"
    );
}

/// Starts a real thread: run it only on a fixture pull request, then stop
/// the thread. `T3_LAUNCH_PR` is `owner/name#number@head-branch`.
#[test]
#[ignore = "starts a thread on a live T3 server: T3_URL, T3_TOKEN_FILE, T3_LAUNCH_PR"]
fn live_launches_a_review_thread_linked_to_the_pr() {
    let token = std::fs::read_to_string(var("T3_TOKEN_FILE")).expect("read T3_TOKEN_FILE");
    let t3 = T3::new(var("T3_URL"), token.trim());
    let spec = var("T3_LAUNCH_PR");
    let (pr, head) = spec
        .split_once('@')
        .expect("T3_LAUNCH_PR must look like owner/name#number@head-branch");
    let pr = parse_pr(pr);

    assert!(t3.session().expect("session").can_operate);
    let project = t3
        .projects()
        .expect("projects")
        .into_iter()
        .find(|project| project.holds(&pr))
        .expect("a T3 project for the repository");
    let model = "claude-sonnet-5-5";
    let instance = t3
        .providers()
        .expect("providers")
        .into_iter()
        .find(|provider| {
            provider.unavailable.is_none()
                && provider.models.iter().any(|offered| offered.slug == model)
        })
        .expect("a provider that offers the model")
        .instance_id;
    let launch = loupe_t3::ReviewLaunch {
        request_id: format!("loupe-live-{}", std::process::id()),
        project_id: project.id.clone(),
        title: format!("Loupe live test: {}#{}", pr.repository, pr.number),
        message: "Loupe live test of the review launch. Reply with \"ok\" and stop; \
                  do not run any command."
            .into(),
        model: loupe_t3::ModelChoice {
            instance_id: instance,
            model: model.into(),
            effort: Some("low".into()),
        },
        base_ref: head.into(),
        branch: format!("review/pr-{}-live-{}", pr.number, std::process::id()),
        pull_request_url: format!("https://github.com/{}/pull/{}", pr.repository, pr.number),
        pull_request: pr.clone(),
    };
    let launched = t3.launch_review(&launch).expect("launch");
    println!(
        "launched {} in {:?}; link: {:?}",
        launched.thread_id, project.title, launched.link_error
    );
    assert!(launched.link_error.is_none());
    let linked = t3.linked_threads(&pr).expect("linked threads");
    assert!(
        linked.iter().any(|thread| thread.id == launched.thread_id),
        "the new thread is not linked to the pull request"
    );
}
