//! Output for `son list`, `son log` and `son stats`.

use crate::activity::{Change, Count, EventRow, Page, Stats};
use crate::history::{EdgeChange, Entry};
use crate::render::{self, node_json, print_json};
use crate::store;
use crate::store::Node;
use serde_json::{json, Value};
use std::path::Path;

const BAR_WIDTH: i64 = 24;

fn page_json<T>(page: &Page<T>, items: Vec<Value>) -> Value {
    json!({
        "page": page.page,
        "pages": page.pages,
        "total": page.total,
        "limit": page.limit,
        "items": items,
    })
}

/// "1 node", "2 nodes".
fn count(n: i64, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// "page 2 of 7 (131 nodes)" and how to get the next one.
fn page_header<T>(title: &str, page: &Page<T>, noun: &str) {
    println!(
        "# {title} · page {} of {} ({})",
        page.page,
        page.pages,
        count(page.total as i64, noun)
    );
}

fn page_footer<T>(page: &Page<T>, command: &str) {
    if page.page < page.pages {
        println!("\nNext page: `{command} --page {}`", page.page + 1);
    }
}

pub fn nodes(page: &Page<Change>, scope: &str, command: &str, json: bool) {
    if json {
        let items = page
            .items
            .iter()
            .map(|change| {
                let mut value = node_json(&change.node);
                value["action"] = json!(change.action);
                value["at"] = json!(change.at);
                value
            })
            .collect();
        print_json(&page_json(page, items));
        return;
    }
    if page.total == 0 {
        println!("Nothing is stored for {scope}.");
        return;
    }
    page_header(&format!("Nodes: {scope}"), page, "node");
    if page.items.is_empty() {
        println!("\nThis page is past the end.");
    }
    for change in &page.items {
        // Minutes are enough here: drop the seconds.
        let at = change.at.get(..16).unwrap_or(&change.at);
        println!("- {at} {:<7} {}", change.action, render::line(&change.node));
    }
    page_footer(page, command);
}

fn event_line(event: &EventRow) -> String {
    let mut line = format!("- {} {}", event.at, event.command);
    if let Some(detail) = &event.detail {
        if event.command == "recall" {
            line.push_str(&format!(" \"{detail}\""));
        } else {
            line.push_str(&format!(" {detail}"));
        }
    }
    if let Some(id) = &event.node_id {
        line.push_str(&format!(" `{}`", store::short_id(id)));
    }
    if let Some(error) = &event.error {
        line.push_str(&format!(" FAILED: {error}"));
    } else if let Some(results) = event.results {
        line.push_str(&format!(
            " → {results} result{}",
            if results == 1 { "" } else { "s" }
        ));
        if let Some(close) = event.close.filter(|c| *c > 0) {
            line.push_str(&format!(" ({close} close)"));
        }
    }
    let mut tail = Vec::new();
    if let Some(project) = &event.project {
        tail.push(project.clone());
    }
    if let Some(source) = &event.source {
        tail.push(format!("session {source}"));
    }
    tail.push(format!("{} ms", event.ms));
    format!("{line} · {}", tail.join(" · "))
}

pub fn events(page: &Page<EventRow>, command: &str, json: bool) {
    if json {
        let items = page
            .items
            .iter()
            .map(|event| serde_json::to_value(event).expect("an event always serializes"))
            .collect();
        print_json(&page_json(page, items));
        return;
    }
    if page.total == 0 {
        println!("No commands have been logged yet.");
        return;
    }
    page_header("Activity log", page, "command");
    if page.items.is_empty() {
        println!("\nThis page is past the end.");
    }
    for event in &page.items {
        println!("{}", event_line(event));
    }
    page_footer(page, command);
}

fn quoted(text: &str) -> String {
    format!("\"{}\"", render::clip(text, 60))
}

/// What an operation changed in a node, in a few words.
fn changes(before: Option<&Node>, after: &Node) -> Vec<String> {
    let Some(before) = before else {
        return vec![format!("created [{}] {}", after.kind, quoted(&after.title))];
    };
    let mut parts = Vec::new();
    if before.deleted_at.is_none() && after.deleted_at.is_some() {
        parts.push("dropped".to_string());
    }
    if before.deleted_at.is_some() && after.deleted_at.is_none() {
        parts.push("restored".to_string());
    }
    if before.title != after.title {
        parts.push(format!(
            "title {} → {}",
            quoted(&before.title),
            quoted(&after.title)
        ));
    }
    if before.body != after.body {
        parts.push(format!(
            "body changed ({} → {} characters)",
            before.body.chars().count(),
            after.body.chars().count()
        ));
    }
    let pair = |name: &str, old: Option<&str>, new: Option<&str>| {
        (old != new).then(|| format!("{name} {} → {}", old.unwrap_or("-"), new.unwrap_or("-")))
    };
    parts.extend(pair("type", Some(&before.kind), Some(&after.kind)));
    parts.extend(pair(
        "status",
        before.status.as_deref(),
        after.status.as_deref(),
    ));
    if before.project_id != after.project_id {
        parts.extend(pair(
            "collection",
            before.project.as_deref(),
            after.project.as_deref(),
        ));
    }
    if before.tags != after.tags {
        parts.push(format!(
            "tags {} → {}",
            if before.tags.is_empty() {
                "-".to_string()
            } else {
                before.tags.join(",")
            },
            if after.tags.is_empty() {
                "-".to_string()
            } else {
                after.tags.join(",")
            }
        ));
    }
    if before.evidence != after.evidence {
        parts.push(format!(
            "evidence {} → {}",
            store::evidence_label(before.evidence.as_deref()),
            store::evidence_label(after.evidence.as_deref())
        ));
    }
    if before.refs != after.refs {
        parts.push(format!(
            "evidence references {} → {}",
            before.refs.len(),
            after.refs.len()
        ));
    }
    if before.verified_at != after.verified_at {
        parts.push(match &after.verified_at {
            Some(_) => "verified".to_string(),
            None => "verification removed".to_string(),
        });
    }
    parts
}

fn link_change(change: &EdgeChange, node: &Node) -> String {
    let sign = if change.added { "linked" } else { "unlinked" };
    if change.from_id == node.id {
        format!(
            "{sign}: {} `{}`",
            change.relation,
            store::short_id(&change.to_id)
        )
    } else {
        format!(
            "{sign}: `{}` {} this",
            store::short_id(&change.from_id),
            change.relation
        )
    }
}

fn entry_json(entry: &Entry) -> Value {
    serde_json::to_value(entry).expect("a history entry always serializes")
}

fn entry_head(entry: &Entry) -> String {
    let mut head = format!(
        "#{} {} {}",
        entry.op,
        entry.at.get(..16).unwrap_or(&entry.at),
        entry.kind
    );
    if let Some(undoes) = entry.undoes {
        head.push_str(&format!(" of #{undoes}"));
    }
    if let Some(who) = entry.source.as_ref().or(entry.author.as_ref()) {
        head.push_str(&format!(" by {who}"));
    }
    head
}

fn entry_notes(entry: &Entry) -> String {
    let mut notes = String::new();
    if entry.other_nodes > 0 {
        notes.push_str(&format!(" (+{} other node(s))", entry.other_nodes));
    }
    if let Some(by) = entry.undone_by {
        notes.push_str(&format!(" (undone by #{by})"));
    }
    if entry.purged {
        notes.push_str(" (part purged; cannot be undone)");
    }
    notes
}

/// `son history <id>`: every operation that changed the node or its links.
pub fn history(node: &Node, page: &Page<Entry>, command: &str, json: bool) {
    if json {
        let items = page.items.iter().map(entry_json).collect();
        let mut value = page_json(page, items);
        value["node"] = node_json(node);
        print_json(&value);
        return;
    }
    if page.total == 0 {
        println!(
            "No changes are recorded for {}.\nHistory is kept from son 0.1.6 on; a change made before that is not listed.",
            render::line(node)
        );
        return;
    }
    page_header(&format!("History: {}", render::line(node)), page, "change");
    if page.items.is_empty() {
        println!("\nThis page is past the end.");
    }
    for entry in &page.items {
        let mut what = match &entry.after {
            Some(after) => changes(entry.before.as_ref(), after),
            None => Vec::new(),
        };
        what.extend(entry.links.iter().map(|change| link_change(change, node)));
        println!(
            "- {}: {}{}",
            entry_head(entry),
            what.join("; "),
            entry_notes(entry)
        );
    }
    println!(
        "\n`son history {} --op <number>` shows one change in full; `son undo {}` reverses the newest.",
        node.short(),
        node.short()
    );
    page_footer(page, command);
}

fn snapshot(label: &str, node: &Node) {
    println!("\n## {label}");
    let mut facts = vec![
        format!("[{}]", node.kind),
        format!("collection: {}", node.project_name()),
        format!("rev {}", node.rev),
    ];
    if let Some(status) = &node.status {
        facts.push(format!("status: {status}"));
    }
    if !node.tags.is_empty() {
        facts.push(format!("tags: {}", node.tags.join(", ")));
    }
    facts.extend(render::trust(node));
    if node.deleted_at.is_some() {
        facts.push("DROPPED".to_string());
    }
    println!("{}\n{}", node.title, facts.join(" · "));
    if !node.body.trim().is_empty() {
        println!("\n{}", node.body.trim());
    }
    for r in &node.refs {
        println!("- evidence {}: {}", r.kind, r.value);
    }
}

/// `son history <id> --op <n>`: one operation, with the node in full
/// before it and after it.
pub fn history_entry(node: &Node, entry: &Entry, json: bool) {
    if json {
        let mut value = entry_json(entry);
        value["node"] = node_json(node);
        print_json(&value);
        return;
    }
    println!("# {}{}", entry_head(entry), entry_notes(entry));
    println!("{}", render::line(node));
    match (&entry.before, &entry.after) {
        (before, Some(after)) => {
            match before {
                Some(before) => snapshot("Before", before),
                None => println!("\n## Before\n(the node did not exist)"),
            }
            snapshot("After", after);
        }
        _ => println!("\nThe node itself did not change."),
    }
    if !entry.links.is_empty() {
        println!("\n## Links");
        for change in &entry.links {
            println!("- {}", link_change(change, node));
        }
    }
}

fn percent(share: Option<f64>) -> String {
    share.map_or("-".to_string(), |s| format!("{:.0}%", s * 100.0))
}

fn inline(counts: &[Count]) -> String {
    counts
        .iter()
        .map(|c| format!("{} {}", c.name, c.count))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn size(bytes: u64) -> String {
    match bytes {
        0..=1023 => format!("{bytes} B"),
        1024..=1_048_575 => format!("{:.0} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

/// What the numbers say about how well `son` is being used.
fn remarks(stats: &Stats, set_up: bool) -> Vec<String> {
    let mut remarks = Vec::new();
    let (activity, recall) = (&stats.activity, &stats.recall);
    if !set_up {
        remarks.push(
            "son is not set up for any agent here, so no agent has a reason to call it. Run `son init`."
                .to_string(),
        );
    }
    if activity.commands == 0 {
        remarks.push("No commands were logged in this period.".to_string());
        return remarks;
    }
    if recall.recalls == 0 {
        remarks.push("Nothing was recalled: sessions are not reading from son.".to_string());
    }
    if activity.writes == 0 {
        remarks.push("Nothing was written: sessions are not saving to son.".to_string());
    }
    if recall.recalls >= 5 {
        if recall.hit_rate.is_some_and(|rate| rate < 0.6) {
            remarks.push(format!(
                "Only {} of recalls found something. The store may be too thin, or the queries need synonyms (`son synonym add`).",
                percent(recall.hit_rate)
            ));
        }
        if activity.writes_per_recall.is_some_and(|ratio| ratio < 0.1) {
            remarks.push(
                "Sessions read much more than they write back. Decisions and patterns may be going unsaved."
                    .to_string(),
            );
        }
    }
    if activity.errors * 5 > activity.commands {
        remarks.push(format!(
            "{} of {} commands failed. See `son log --errors`.",
            activity.errors, activity.commands
        ));
    }
    if stats.store.open_todos_older_than_30_days > 0 {
        remarks.push(format!(
            "{} open todo(s) are older than 30 days.",
            stats.store.open_todos_older_than_30_days
        ));
    }
    if recall.p95_ms.is_some_and(|ms| ms > 100) {
        remarks.push("Some recalls take over 100 ms, which is above the target.".to_string());
    }
    remarks
}

pub fn stats(stats: &Stats, database: &Path, installed: &[std::path::PathBuf], json: bool) {
    let bytes = std::fs::metadata(database).map(|m| m.len()).unwrap_or(0);
    let remarks = remarks(stats, !installed.is_empty());

    if json {
        let mut value = serde_json::to_value(stats).expect("stats always serialize");
        value["database"] = json!({ "path": database.display().to_string(), "bytes": bytes });
        value["installed"] = json!(installed
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>());
        value["remarks"] = json!(remarks);
        print_json(&value);
        return;
    }

    let period = match stats.days {
        Some(days) => format!("last {days} days"),
        None => "all time".to_string(),
    };
    let store = &stats.store;
    println!("# son stats ({period})");

    println!("\n## Store (now)");
    println!(
        "- {} in {}{}",
        count(store.nodes, "node"),
        count(store.projects, "collection"),
        if store.by_type.is_empty() {
            String::new()
        } else {
            format!(": {}", inline(&store.by_type))
        }
    );
    println!(
        "- todos: {} open, {} done, {} dropped",
        store.todos_open, store.todos_done, store.todos_dropped
    );
    println!(
        "- {} links · {} tags · {} synonyms · {} dropped nodes",
        store.links, store.tags, store.synonyms, store.dropped
    );
    if let (Some(first), Some(last)) = (&store.first_node_at, &store.last_change_at) {
        println!("- first node {first} · last change {last}");
    }
    println!("- database: {} ({})", database.display(), size(bytes));
    if installed.is_empty() {
        println!("- set up for agents: NOT set up (run `son init`)");
    } else {
        let places: Vec<String> = installed
            .iter()
            .map(|path| crate::setup::short_path(path))
            .collect();
        println!("- set up for agents in: {}", places.join(", "));
    }

    let activity = &stats.activity;
    println!("\n## Activity ({period})");
    println!(
        "- {} commands on {} day{}: {} reads, {} writes, {} failed",
        activity.commands,
        activity.active_days,
        if activity.active_days == 1 { "" } else { "s" },
        activity.reads,
        activity.writes,
        activity.errors
    );
    if activity.sessions > 0 {
        println!("- {} named sessions (SON_SOURCE)", activity.sessions);
    }
    if let Some(ratio) = activity.writes_per_recall {
        println!("- {ratio:.2} writes for each recall");
    }
    if let Some(first) = &stats.first_event_at {
        println!("- logging since {first}");
    }

    let recall = &stats.recall;
    println!("\n## Recall ({period})");
    if recall.recalls == 0 {
        println!("- no recalls");
    } else {
        println!(
            "- {} recalls: {} found something ({}), {} found nothing",
            recall.recalls,
            recall.found_something,
            percent(recall.hit_rate),
            recall.found_nothing
        );
        println!(
            "- {} needed a close match (start of a word, or a typo)",
            recall.needed_close_match
        );
        if let (Some(average), Some(median), Some(p95)) =
            (recall.average_results, recall.median_ms, recall.p95_ms)
        {
            println!("- {average:.1} results on average · median {median} ms · 95% under {p95} ms");
        }
        if !recall.missed.is_empty() {
            let missed: Vec<String> = recall
                .missed
                .iter()
                .map(|m| format!("\"{}\" ({})", m.name, m.count))
                .collect();
            println!("- found nothing for: {}", missed.join(", "));
        }
    }

    if !stats.by_command.is_empty() {
        println!("\n## Commands\n{}", inline(&stats.by_command));
    }
    if !stats.by_project.is_empty() {
        println!("\n## Collections\n{}", inline(&stats.by_project));
    }
    if !stats.by_day.is_empty() {
        println!("\n## By day");
        let most = stats
            .by_day
            .iter()
            .map(|d| d.commands)
            .max()
            .unwrap_or(1)
            .max(1);
        for day in &stats.by_day {
            let bar = "█".repeat(((day.commands * BAR_WIDTH + most - 1) / most) as usize);
            println!(
                "{} {bar} {} ({} recalls, {} writes)",
                day.day, day.commands, day.recalls, day.writes
            );
        }
    }

    if !remarks.is_empty() {
        println!("\n## Worth a look");
        for remark in &remarks {
            println!("- {remark}");
        }
    }
}
