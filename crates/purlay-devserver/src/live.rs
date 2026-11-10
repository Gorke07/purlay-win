//! Live mode: the seeded bot keeps #random moving, so a connected client can be watched
//! updating. Every round it types for a moment, then posts, replies in a thread, reacts,
//! or mentions whoever signed in last.

use crate::http::Shared;
use crate::ids::new_id;
use crate::world::chat::PostInput;
use purlay_contract::Uuid;
use std::time::Duration;

const LINES: &[&str] = &[
    "Deploy finished. Nothing is on fire. 🔥🚫",
    "Reminder: hydrate. Robots need oil, humans need water.",
    "I just ran the test suite for fun. 412 passed. I feel great.",
    "Coffee machine status: **operational** ☕",
    "Did you know? The first computer bug was an actual moth (1947).",
    "Today's build took `10m 41s`. A new record, almost.",
    "Here is a haiku:\n\n> green checks in a row\n> the merge queue hums quietly\n> someone broke the build",
    "Quick poll: tabs or spaces? React with 1️⃣ or 2️⃣.",
    "I have updated 3 dependencies. They did not complain.",
    "Weekend forecast: 100% chance of no deploys.",
    "Fun fact: `0.1 + 0.2 != 0.3`. I checked twice.",
    "Uptime this week: 99.98%. The 0.02% was a nap.",
];

const MENTIONS: &[&str] = &[
    "can you take a look at the release notes when you have a minute?",
    "your review is the last one missing on PR #1234 🙏",
    "standup moved to 10:15 today.",
    "the staging deploy you asked for is ready.",
    "you have a build waiting for approval.",
];

const REPLIES: &[&str] = &[
    "Agreed!",
    "I can confirm this from the logs.",
    "+1",
    "Looking into it now.",
    "Done ✅",
    "Interesting. I'll add it to the list.",
];

const EMOJI: &[&str] = &["👍", "🎉", "🚀", "😂", "👀", "❤️"];

fn pick<'a>(items: &'a [&'a str]) -> &'a str {
    items[(rand::random::<u32>() as usize) % items.len()]
}

pub async fn run(app: Shared, every: Duration) {
    let (bot, random) = {
        let w = app.world.lock();
        let Some(bot) = w.bot.clone() else { return };
        let Some(random) = w.channel_by_name("random").map(|c| c.id.clone()) else { return };
        (bot, random)
    };
    tracing::info!("live mode: the bot posts in #random about every {} s", every.as_secs());
    loop {
        let jitter = 0.75 + rand::random::<f64>() * 0.5;
        tokio::time::sleep(every.mul_f64(jitter)).await;
        let roll = rand::random::<f64>();
        if roll < 0.2 {
            react(&app, &bot, &random);
            continue;
        }
        app.world.lock().publish_typing(&random, &bot);
        tokio::time::sleep(Duration::from_millis(2500)).await;
        if roll < 0.45 {
            reply(&app, &bot, &random);
        } else {
            post(&app, &bot, &random, roll < 0.7);
        }
    }
}

fn post(app: &Shared, bot: &Uuid, channel: &Uuid, mention: bool) {
    let mut w = app.world.lock();
    let content = if mention {
        let who = w.audience();
        let name = w.person(&who).map(|p| p.username.clone()).unwrap_or_else(|| "ada".into());
        format!("@{name} {}", pick(MENTIONS))
    } else {
        pick(LINES).to_string()
    };
    w.post(PostInput {
        id: new_id(),
        channel_id: channel.clone(),
        sender: bot.clone(),
        content,
        parent_id: None,
        attachment_ids: Vec::new(),
        at_ms: crate::ids::now_ms(),
    });
}

fn recent_roots(w: &crate::world::World, channel: &Uuid, n: usize) -> Vec<Uuid> {
    w.channels
        .get(channel)
        .map(|c| {
            c.roots
                .iter()
                .rev()
                .filter(|id| w.messages.get(*id).is_some_and(|m| m.deleted_ms.is_none()))
                .take(n)
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn reply(app: &Shared, bot: &Uuid, channel: &Uuid) {
    let mut w = app.world.lock();
    let roots = recent_roots(&w, channel, 5);
    let Some(root) = roots.get((rand::random::<u32>() as usize) % roots.len().max(1)).cloned() else { return };
    w.post(PostInput {
        id: new_id(),
        channel_id: channel.clone(),
        sender: bot.clone(),
        content: pick(REPLIES).to_string(),
        parent_id: Some(root),
        attachment_ids: Vec::new(),
        at_ms: crate::ids::now_ms(),
    });
}

fn react(app: &Shared, bot: &Uuid, channel: &Uuid) {
    let mut w = app.world.lock();
    let roots: Vec<Uuid> = recent_roots(&w, channel, 5)
        .into_iter()
        .filter(|id| w.messages.get(id).is_some_and(|m| m.sender.as_ref() != Some(bot)))
        .collect();
    let Some(target) = roots.first().cloned() else { return };
    let _ = w.put_reaction(bot, &target, pick(EMOJI), true);
}
