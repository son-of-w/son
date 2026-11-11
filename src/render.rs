//! Output: compact markdown by default, JSON with `--json`.

use crate::search::Recall;
use crate::store::{Edge, Node, ProjectRow};
use serde_json::{json, Value};

const HIT_BODY_CHARS: usize = 800;

pub fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("a JSON value always serializes")
    );
}

pub fn node_json(node: &Node) -> Value {
    let mut value = serde_json::to_value(node).expect("a node always serializes");
    value["short_id"] = json!(node.short());
    value
}

fn nodes_json(nodes: &[Node]) -> Value {
    Value::Array(nodes.iter().map(node_json).collect())
}

/// The date part of a timestamp.
fn date(timestamp: &str) -> &str {
    timestamp.get(..10).unwrap_or(timestamp)
}

/// `text` cut to `max` characters, with a mark when something was cut.
pub fn clip(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(max).collect();
    cut.push('…');
    cut
}

fn first_line(text: &str) -> &str {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
}

/// How far a node can be trusted, in a few words: how it was learned and
/// whether it was checked. Empty when neither is known.
pub fn trust(node: &Node) -> Vec<String> {
    let mut parts = Vec::new();
    if node.evidence.is_some() {
        parts.push(crate::store::evidence_label(node.evidence.as_deref()).to_string());
    }
    if let Some(when) = &node.verified_at {
        parts.push(format!("verified {}", date(when)));
    }
    parts
}

/// [`trust`] as a suffix for a one-line summary.
fn trust_suffix(node: &Node) -> String {
    let parts = trust(node);
    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
    }
}

fn tags(node: &Node) -> String {
    node.tags.iter().map(|t| format!(" #{t}")).collect()
}

/// One line for a node: `` `id` [type] title (collection) #tags ``.
pub fn line(node: &Node) -> String {
    let status = match (node.kind.as_str(), node.status.as_deref()) {
        ("todo", Some(status)) => format!(" [{status}]"),
        _ => String::new(),
    };
    format!(
        "`{}` [{}]{status} {} ({}){}",
        node.short(),
        node.kind,
        node.title,
        node.project_name(),
        tags(node)
    )
}

pub fn saved(verb: &str, node: &Node, json: bool) {
    if json {
        print_json(&node_json(node));
    } else {
        println!("{verb} {}", line(node));
    }
}

pub fn show(node: &Node, edges: &[Edge], superseded_by: Option<&Node>, json: bool) {
    if json {
        let mut value = node_json(node);
        value["links"] = Value::Array(
            edges
                .iter()
                .map(|e| json!({ "relation": e.label(), "node": node_json(&e.node) }))
                .collect(),
        );
        value["superseded_by"] = json!(superseded_by.map(Node::short));
        print_json(&value);
        return;
    }

    println!("# {}", node.title);
    let mut facts = vec![
        format!("`{}`", node.short()),
        node.kind.clone(),
        format!("collection: {}", node.project_name()),
    ];
    if let Some(status) = &node.status {
        facts.push(format!("status: {status}"));
    }
    if !node.tags.is_empty() {
        facts.push(format!("tags: {}", node.tags.join(", ")));
    }
    println!("{}", facts.join(" · "));
    println!(
        "created {} · updated {} · full id {}",
        date(&node.created_at),
        date(&node.updated_at),
        node.id.to_lowercase()
    );
    let verified = match (&node.verified_at, &node.verified_by) {
        (Some(when), Some(by)) => format!("verified {} by {by}", date(when)),
        (Some(when), None) => format!("verified {}", date(when)),
        (None, _) => "not verified".to_string(),
    };
    if node.kind != "collection" {
        println!(
            "evidence: {} · {verified}",
            crate::store::evidence_label(node.evidence.as_deref())
        );
        if let Some(note) = node.verified_note.as_deref().filter(|n| !n.is_empty()) {
            println!("checked: {note}");
        }
    }
    if let Some(when) = &node.deleted_at {
        println!(
            "DROPPED {} (bring it back with `son restore {}`)",
            date(when),
            node.short()
        );
    }
    if let Some(newer) = superseded_by {
        println!("SUPERSEDED by `{}` {}", newer.short(), newer.title);
    }
    if !node.body.trim().is_empty() {
        println!("\n{}", node.body.trim());
    }
    if !node.refs.is_empty() {
        println!("\n## Evidence");
        for r in &node.refs {
            println!("- {}: {}", r.kind, r.value);
        }
    }
    if !edges.is_empty() {
        println!("\n## Links");
        for edge in edges {
            println!("- {} {}", edge.label(), line(&edge.node));
        }
    }
}

pub fn recall(terms: &str, current_project: &str, result: &Recall, json: bool) {
    if json {
        let hits: Vec<Value> = result
            .hits
            .iter()
            .map(|h| {
                let mut value = node_json(&h.node);
                value["score"] = json!((h.score * 1000.0).round() / 1000.0);
                value["current_collection"] = json!(h.current);
                value["match"] = json!(if h.close { "close" } else { "exact" });
                value["superseded_by"] = json!(h.superseded_by);
                value
            })
            .collect();
        let neighbors: Vec<Value> = result
            .neighbors
            .iter()
            .map(|n| json!({ "relation": n.relation, "of": n.of, "node": node_json(&n.node) }))
            .collect();
        print_json(&json!({
            "query": terms,
            "current_collection": current_project,
            "hits": hits,
            "linked": neighbors,
            "also": nodes_json(&result.also),
        }));
        return;
    }

    if result.hits.is_empty() {
        println!("No matches for \"{terms}\" in any collection.");
        return;
    }
    println!("# Recall: {terms} (current collection: {current_project})");
    for hit in &result.hits {
        let node = &hit.node;
        println!("\n## `{}` [{}] {}", node.short(), node.kind, node.title);
        let mut facts = vec![format!(
            "collection: {}{}",
            node.project_name(),
            if hit.current { " (current)" } else { "" }
        )];
        if let Some(status) = &node.status {
            facts.push(format!("status: {status}"));
        }
        if !node.tags.is_empty() {
            facts.push(format!("tags: {}", node.tags.join(", ")));
        }
        facts.push(format!("updated {}", date(&node.updated_at)));
        facts.extend(trust(node));
        if hit.close {
            facts.push("close match".to_string());
        }
        println!("{}", facts.join(" · "));
        if let Some(newer) = &hit.superseded_by {
            println!("SUPERSEDED by `{newer}`; kept for reference.");
        }
        if !node.body.trim().is_empty() {
            let body = clip(&node.body, HIT_BODY_CHARS);
            println!("\n{body}");
            if body.ends_with('…') {
                println!("(cut; `son show {}` for the rest)", node.short());
            }
        }
    }
    if !result.neighbors.is_empty() {
        println!("\n## Linked");
        for neighbor in &result.neighbors {
            println!(
                "- `{}` {} {}",
                neighbor.of,
                neighbor.relation,
                line(&neighbor.node)
            );
        }
    }
    if !result.also.is_empty() {
        println!("\n## Also in these collections");
        for node in &result.also {
            println!("- {}", line(node));
        }
    }
}

fn todo_box(node: &Node) -> &'static str {
    match node.status.as_deref() {
        Some("done") => "[x]",
        Some("dropped") => "[-]",
        _ => "[ ]",
    }
}

pub fn todos(nodes: &[Node], scope: &str, json: bool) {
    if json {
        print_json(&nodes_json(nodes));
        return;
    }
    if nodes.is_empty() {
        println!("No todos for {scope}.");
        return;
    }
    println!("# Todos: {scope}");
    for node in nodes {
        println!(
            "- {} `{}` {} ({}){}",
            todo_box(node),
            node.short(),
            node.title,
            node.project_name(),
            tags(node)
        );
    }
}

pub fn projects(rows: &[ProjectRow], current: &str, json: bool) {
    if json {
        let list: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut value = node_json(&row.node);
                value["nodes"] = json!(row.nodes);
                value["open_todos"] = json!(row.open_todos);
                value
            })
            .collect();
        print_json(&Value::Array(list));
        return;
    }
    if rows.is_empty() {
        println!("No collections yet. The first `son remember` creates one.");
        return;
    }
    println!("# Collections");
    for row in rows {
        let mark = if row.node.title.eq_ignore_ascii_case(current) {
            " (current)"
        } else {
            ""
        };
        println!(
            "- `{}` {}{mark} · {} nodes · {} open todos",
            row.node.short(),
            row.node.title,
            row.nodes,
            row.open_todos
        );
    }
}

/// What a collection holds, grouped for the start of a session.
pub struct Context<'a> {
    pub project: &'a Node,
    pub patterns: Vec<Node>,
    pub decisions: Vec<Node>,
    pub facts: Vec<Node>,
    pub todos: Vec<Node>,
    pub notes: Vec<Node>,
}

pub fn context(ctx: &Context, json: bool) {
    if json {
        print_json(&json!({
            "collection": node_json(ctx.project),
            "patterns": nodes_json(&ctx.patterns),
            "decisions": nodes_json(&ctx.decisions),
            "facts": nodes_json(&ctx.facts),
            "todos": nodes_json(&ctx.todos),
            "notes": nodes_json(&ctx.notes),
        }));
        return;
    }

    println!("# Context: {}", ctx.project.title);
    if !ctx.project.body.trim().is_empty() {
        println!("\n{}", ctx.project.body.trim());
    }
    if !ctx.patterns.is_empty() {
        println!("\n## Patterns (how we set things up)");
        for node in &ctx.patterns {
            println!(
                "\n### `{}` {}{}{}",
                node.short(),
                node.title,
                tags(node),
                trust_suffix(node)
            );
            if !node.body.trim().is_empty() {
                println!("{}", clip(&node.body, 600));
            }
        }
    }
    let summary = |node: &Node| {
        let detail = clip(first_line(&node.body), 200);
        if detail.is_empty() {
            format!(
                "- `{}` {}{}{}",
                node.short(),
                node.title,
                tags(node),
                trust_suffix(node)
            )
        } else {
            format!(
                "- `{}` {}: {detail}{}{}",
                node.short(),
                node.title,
                tags(node),
                trust_suffix(node)
            )
        }
    };
    for (heading, nodes) in [
        ("Decisions", &ctx.decisions),
        ("Facts", &ctx.facts),
        ("Recent notes", &ctx.notes),
    ] {
        if !nodes.is_empty() {
            println!("\n## {heading}");
            for node in nodes {
                println!("{}", summary(node));
            }
        }
    }
    if !ctx.todos.is_empty() {
        println!("\n## Open todos");
        for node in &ctx.todos {
            println!("- [ ] `{}` {}{}", node.short(), node.title, tags(node));
        }
    }
    println!(
        "\n(`son recall <terms>` searches every collection; `son show <id>` prints a node in full.)"
    );
}
