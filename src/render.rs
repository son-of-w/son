//! Output: compact markdown by default, JSON with `--json`.

use crate::search::{Candidate, Recall};
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
    if node.stale_at.is_some() {
        parts.push("STALE".to_string());
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

/// `son remember --check`: what a new node may duplicate. Nothing is saved,
/// and the output says so in a way a script cannot miss.
pub fn check(kind: &str, title: &str, collection: &str, candidates: &[Candidate], json: bool) {
    if json {
        let list: Vec<Value> = candidates
            .iter()
            .map(|c| {
                let mut value = node_json(&c.node);
                value["score"] = json!((c.score * 100.0).round() / 100.0);
                value["reasons"] = json!(c.reasons);
                value
            })
            .collect();
        print_json(&json!({
            "check": true,
            "saved": false,
            "type": kind,
            "title": title,
            "collection": collection,
            "candidates": list,
        }));
        return;
    }
    println!("# Check: [{kind}] {title} (collection: {collection})");
    println!("NOT SAVED. This only looked for stored nodes that may say the same thing.");
    if candidates.is_empty() {
        println!("\nNo likely duplicate. Run the command again without --check to save it.");
        return;
    }
    println!("\n## Possible duplicates");
    for c in candidates {
        println!("- {} · {}", line(&c.node), c.reasons.join("; "));
    }
    println!(
        "\nAlike words are a hint, not proof. Read a candidate with `son show <id>`. \
         If it is the same knowledge, change it with `son edit <id>`. \
         If it is not, run the command again without --check."
    );
}

pub fn show(
    node: &Node,
    edges: &[Edge],
    superseded_by: Option<&Node>,
    merged_from: &[String],
    json: bool,
) {
    if json {
        let mut value = node_json(node);
        value["merged_from"] = json!(merged_from);
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
    if let Some(when) = &node.stale_at {
        let note = node.stale_note.as_deref().unwrap_or("").trim();
        println!(
            "STALE since {}{}{} (clear with `son verify` or `son stale {} --clear`)",
            date(when),
            if note.is_empty() { "" } else { ": " },
            note,
            node.short()
        );
    }
    if let Some(when) = &node.deleted_at {
        println!(
            "DROPPED {} (bring it back with `son restore {}`)",
            date(when),
            node.short()
        );
    }
    if let Some(into) = &node.merged_into {
        println!(
            "MERGED into `{}` (see `son show {}`)",
            crate::store::short_id(into),
            crate::store::short_id(into)
        );
    }
    if !merged_from.is_empty() {
        let ids: Vec<String> = merged_from
            .iter()
            .map(|id| format!("`{}`", crate::store::short_id(id)))
            .collect();
        println!("merged from {}", ids.join(", "));
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

/// What `--max-chars` and `--explain` ask for.
#[derive(Default)]
pub struct Budget {
    /// The most characters the whole answer may have. A character is one
    /// Unicode code point; line ends count.
    pub max_chars: Option<usize>,
    pub explain: bool,
}

/// How many characters `text` has, by the rule of `--max-chars`.
pub fn chars(text: &str) -> usize {
    text.chars().count()
}

/// A JSON value as the text to print: indented, or on one line when the
/// answer has a budget (indentation would use it up).
pub fn json_text(value: &Value, one_line: bool) -> String {
    let text = if one_line {
        value.to_string()
    } else {
        serde_json::to_string_pretty(value).expect("a JSON value always serializes")
    };
    format!("{text}\n")
}

/// `text` if it fits the budget, else an error. For an answer that has
/// only one form.
pub fn within(text: String, max_chars: Option<usize>) -> anyhow::Result<String> {
    match max_chars {
        Some(max) if chars(&text) > max => Err(too_small(max, chars(&text))),
        _ => Ok(text),
    }
}

fn too_small(max: usize, needed: usize) -> anyhow::Error {
    crate::error::coded(
        "invalid_argument",
        format!(
            "--max-chars {max} is too small: the shortest answer needs {needed} characters. \
             Nothing was printed"
        ),
    )
}

/// The first of `forms` that fits within `max` characters. The forms go
/// from the fullest to the shortest.
fn first_that_fits(forms: impl Iterator<Item = String>, max: usize) -> anyhow::Result<String> {
    let mut shortest = usize::MAX;
    for text in forms {
        let size = chars(&text);
        if size <= max {
            return Ok(text);
        }
        shortest = shortest.min(size);
    }
    Err(too_small(max, shortest))
}

/// A node in a few fields, for an answer that has to be short.
fn brief_json(node: &Node) -> Value {
    json!({
        "short_id": node.short(),
        "type": node.kind,
        "title": node.title,
        "collection": node.project_name(),
    })
}

/// How much of a recall to print. The first shape is everything; each
/// later one is shorter.
#[derive(Clone, Copy)]
struct RecallShape {
    /// Characters of each body.
    body: usize,
    hits: usize,
    linked: usize,
    also: usize,
    /// One line for each hit.
    compact: bool,
    /// Ids only.
    bare: bool,
    /// The answer has a budget.
    budget: bool,
}

fn recall_shapes(result: &Recall, full_body: usize) -> Vec<RecallShape> {
    let mut shape = RecallShape {
        body: full_body,
        hits: result.hits.len(),
        linked: result.neighbors.len(),
        also: result.also.len(),
        compact: false,
        bare: false,
        budget: true,
    };
    let mut shapes = vec![shape];
    shape.also = 0;
    shapes.push(shape);
    shape.linked = 0;
    shapes.push(shape);
    for body in [400, 200, 100, 50, 0] {
        if body < shape.body {
            shape.body = body;
            shapes.push(shape);
        }
    }
    shape.compact = true;
    shapes.push(shape);
    for hits in (1..result.hits.len()).rev() {
        shape.hits = hits;
        shapes.push(shape);
    }
    shape.bare = true;
    for hits in (1..=result.hits.len()).rev() {
        shape.hits = hits;
        shapes.push(shape);
    }
    shapes
}

/// What a shape leaves out, in words; empty when it leaves nothing out.
fn left_out(result: &Recall, shape: &RecallShape, bodies_cut: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut count = |all: usize, kept: usize, noun: &str| {
        if kept < all {
            out.push(format!("{} {noun}", all - kept));
        }
    };
    count(result.hits.len(), shape.hits, "more match(es)");
    count(result.neighbors.len(), shape.linked, "linked node(s)");
    count(
        result.also.len(),
        shape.also,
        "other node(s) of these collections",
    );
    if bodies_cut {
        out.push("bodies cut or left out".to_string());
    }
    out
}

fn factor_text(why: &crate::search::Why, score: f64) -> String {
    let mut text = format!("score {score:.2} = text match {:.2}", why.text_score);
    for factor in &why.factors {
        text.push_str(&format!(" × {} {:.2}", factor.name, factor.times));
    }
    text
}

fn match_text(why: &crate::search::Why) -> String {
    let parts: Vec<String> = why
        .matches
        .iter()
        .map(|m| match m.how {
            "exact" => format!("\"{}\" in {}", m.word, m.field),
            how => format!("\"{}\" in {} as \"{}\" ({how})", m.word, m.field, m.found),
        })
        .collect();
    parts.join("; ")
}

fn recall_markdown(
    terms: &str,
    current: &str,
    result: &Recall,
    shape: &RecallShape,
    explain: bool,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    if result.hits.is_empty() {
        let _ = writeln!(out, "No matches for \"{terms}\" in any collection.");
        return out;
    }
    let hits = &result.hits[..shape.hits];
    if shape.bare {
        let ids: Vec<String> = hits
            .iter()
            .map(|h| format!("`{}`", h.node.short()))
            .collect();
        let _ = writeln!(out, "{} (cut; `son show <id>`)", ids.join(" "));
        return out;
    }
    let _ = writeln!(out, "# Recall: {terms} (current collection: {current})");
    let mut bodies_cut = false;
    for hit in hits {
        let node = &hit.node;
        if shape.compact {
            bodies_cut |= !node.body.trim().is_empty();
            let _ = writeln!(out, "- {}", line(node));
            continue;
        }
        let _ = writeln!(
            out,
            "\n## `{}` [{}] {}",
            node.short(),
            node.kind,
            node.title
        );
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
        let _ = writeln!(out, "{}", facts.join(" · "));
        if let Some(newer) = &hit.superseded_by {
            let _ = writeln!(out, "SUPERSEDED by `{newer}`; kept for reference.");
        }
        if explain {
            let _ = writeln!(out, "why: matched {}", match_text(&hit.why));
            let _ = writeln!(
                out,
                "why: {}{}",
                factor_text(&hit.why, hit.score),
                match (hit.current, hit.close) {
                    (true, true) => "; listed with the current collection, after its exact matches",
                    (true, false) => "; listed first because it is in the current collection",
                    (false, true) => "; listed after exact matches because it is a close match",
                    (false, false) => "",
                }
            );
        }
        if !node.body.trim().is_empty() {
            if shape.body == 0 {
                bodies_cut = true;
                let _ = writeln!(out, "(body left out; `son show {}`)", node.short());
            } else {
                let body = clip(&node.body, shape.body);
                let _ = writeln!(out, "\n{body}");
                if body.ends_with('…') {
                    bodies_cut = true;
                    let _ = writeln!(out, "(cut; `son show {}` for the rest)", node.short());
                }
            }
        }
    }
    if shape.linked > 0 {
        let _ = writeln!(out, "\n## Linked");
        for neighbor in &result.neighbors[..shape.linked] {
            let _ = writeln!(
                out,
                "- `{}` {} {}",
                neighbor.of,
                neighbor.relation,
                line(&neighbor.node)
            );
        }
        if explain {
            let _ = writeln!(
                out,
                "why: each is linked to the match named at the start of its line."
            );
        }
    }
    if shape.also > 0 {
        let _ = writeln!(out, "\n## Also in these collections");
        for node in &result.also[..shape.also] {
            let _ = writeln!(out, "- {}", line(node));
        }
        if explain {
            let _ = writeln!(
                out,
                "why: patterns, decisions and open todos from the collections of the matches."
            );
        }
    }
    if shape.budget {
        let gone = left_out(result, shape, bodies_cut);
        if !gone.is_empty() {
            let _ = writeln!(
                out,
                "\n(shortened to fit --max-chars: {}. `son show <id>` prints a node in full.)",
                gone.join(", ")
            );
        }
    }
    out
}

fn recall_json(
    terms: &str,
    current: &str,
    result: &Recall,
    shape: &RecallShape,
    explain: bool,
) -> Value {
    let hits = &result.hits[..shape.hits];
    if shape.bare {
        let ids: Vec<Value> = hits
            .iter()
            .map(|h| json!({ "short_id": h.node.short() }))
            .collect();
        return json!({ "truncated": true, "hits": ids });
    }
    let mut bodies_cut = false;
    let hits: Vec<Value> = hits
        .iter()
        .map(|h| {
            let mut value = if shape.compact {
                brief_json(&h.node)
            } else {
                node_json(&h.node)
            };
            if shape.budget {
                let body = if shape.compact {
                    String::new()
                } else {
                    clip(&h.node.body, shape.body)
                };
                let cut = body.trim() != h.node.body.trim();
                bodies_cut |= cut;
                if !shape.compact {
                    value["body"] = json!(body);
                }
                value["body_cut"] = json!(cut);
            }
            if !shape.compact {
                value["score"] = json!((h.score * 1000.0).round() / 1000.0);
                value["superseded_by"] = json!(h.superseded_by);
            }
            value["current_collection"] = json!(h.current);
            value["match"] = json!(if h.close { "close" } else { "exact" });
            if explain {
                value["why"] = serde_json::to_value(&h.why).expect("an explanation serializes");
            }
            value
        })
        .collect();
    let node = |n: &Node| {
        if shape.budget {
            brief_json(n)
        } else {
            node_json(n)
        }
    };
    let neighbors: Vec<Value> = result.neighbors[..shape.linked]
        .iter()
        .map(|n| json!({ "relation": n.relation, "of": n.of, "node": node(&n.node) }))
        .collect();
    let also: Vec<Value> = result.also[..shape.also].iter().map(node).collect();
    let mut value = json!({
        "query": terms,
        "current_collection": current,
        "hits": hits,
        "linked": neighbors,
        "also": also,
    });
    if shape.budget {
        let gone = left_out(result, shape, bodies_cut);
        value["truncated"] = json!(!gone.is_empty());
        value["omitted"] = json!({
            "hits": result.hits.len() - shape.hits,
            "linked": result.neighbors.len() - shape.linked,
            "also": result.also.len() - shape.also,
            "bodies_cut": bodies_cut,
        });
    }
    value
}

/// The text of a recall. Without a budget it is the full answer. With one,
/// it is the fullest form that fits; if not even the ids fit, an error.
pub fn recall(
    terms: &str,
    current: &str,
    result: &Recall,
    json: bool,
    budget: &Budget,
) -> anyhow::Result<String> {
    let full_body = if json { usize::MAX } else { HIT_BODY_CHARS };
    let render = |shape: &RecallShape| {
        if json {
            json_text(
                &recall_json(terms, current, result, shape, budget.explain),
                shape.budget,
            )
        } else {
            recall_markdown(terms, current, result, shape, budget.explain)
        }
    };
    let mut shapes = recall_shapes(result, full_body);
    match budget.max_chars {
        None => {
            shapes[0].budget = false;
            Ok(render(&shapes[0]))
        }
        Some(max) => first_that_fits(shapes.iter().map(render), max),
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

/// How much of a context to print.
#[derive(Clone, Copy)]
struct ContextShape {
    /// Characters of each pattern body and each summary line.
    detail: usize,
    /// Items in each section.
    items: usize,
    /// Counts only.
    bare: bool,
    /// The collection's name only.
    minimal: bool,
    budget: bool,
}

fn context_shapes() -> Vec<ContextShape> {
    let mut shape = ContextShape {
        detail: 600,
        items: usize::MAX,
        bare: false,
        minimal: false,
        budget: true,
    };
    let mut shapes = vec![shape];
    for detail in [300, 150, 60, 0] {
        shape.detail = detail;
        shapes.push(shape);
    }
    for items in [8, 5, 3, 2, 1] {
        shape.items = items;
        shapes.push(shape);
    }
    shape.bare = true;
    shapes.push(shape);
    shape.minimal = true;
    shapes.push(shape);
    shapes
}

fn context_sections<'a>(ctx: &'a Context) -> [(&'static str, &'static str, &'a [Node]); 5] {
    [
        ("patterns", "Patterns (how we set things up)", &ctx.patterns),
        ("decisions", "Decisions", &ctx.decisions),
        ("facts", "Facts", &ctx.facts),
        ("notes", "Recent notes", &ctx.notes),
        ("todos", "Open todos", &ctx.todos),
    ]
}

fn context_markdown(ctx: &Context, shape: &ContextShape) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let sections = context_sections(ctx);
    if shape.minimal {
        let _ = writeln!(out, "# Context: {} (cut)", ctx.project.title);
        return out;
    }
    let _ = writeln!(out, "# Context: {}", ctx.project.title);
    if shape.bare {
        let counts: Vec<String> = sections
            .iter()
            .filter(|(_, _, nodes)| !nodes.is_empty())
            .map(|(name, _, nodes)| format!("{} {name}", nodes.len()))
            .collect();
        let _ = writeln!(
            out,
            "{} (cut; `son list --collection {}`)",
            counts.join(", "),
            ctx.project.title
        );
        return out;
    }
    let mut cut = false;
    let mut left_out = 0;
    let about = ctx.project.body.trim();
    if !about.is_empty() {
        if shape.detail == 0 {
            cut = true;
        } else {
            let text = if shape.budget {
                clip(about, shape.detail)
            } else {
                about.to_string()
            };
            cut |= text != about;
            let _ = writeln!(out, "\n{text}");
        }
    }
    for (name, heading, nodes) in sections {
        if nodes.is_empty() {
            continue;
        }
        let shown = &nodes[..nodes.len().min(shape.items)];
        left_out += nodes.len() - shown.len();
        let _ = writeln!(out, "\n## {heading}");
        for node in shown {
            match name {
                "patterns" => {
                    let _ = writeln!(
                        out,
                        "\n### `{}` {}{}{}",
                        node.short(),
                        node.title,
                        tags(node),
                        trust_suffix(node)
                    );
                    let body = node.body.trim();
                    if !body.is_empty() && shape.detail > 0 {
                        let text = clip(body, shape.detail);
                        cut |= shape.budget && text != body;
                        let _ = writeln!(out, "{text}");
                    } else if !body.is_empty() {
                        cut = true;
                    }
                }
                "todos" => {
                    let _ = writeln!(out, "- [ ] `{}` {}{}", node.short(), node.title, tags(node));
                }
                _ => {
                    let first = first_line(&node.body);
                    let detail = clip(first, shape.detail.min(200));
                    cut |= shape.budget && detail != first;
                    if detail.is_empty() || shape.detail == 0 {
                        let _ = writeln!(
                            out,
                            "- `{}` {}{}{}",
                            node.short(),
                            node.title,
                            tags(node),
                            trust_suffix(node)
                        );
                    } else {
                        let _ = writeln!(
                            out,
                            "- `{}` {}: {detail}{}{}",
                            node.short(),
                            node.title,
                            tags(node),
                            trust_suffix(node)
                        );
                    }
                }
            }
        }
    }
    if !shape.budget {
        let _ = writeln!(
            out,
            "\n(`son recall <terms>` searches every collection; `son show <id>` prints a node in full.)"
        );
    } else if cut || left_out > 0 {
        let mut gone = Vec::new();
        if left_out > 0 {
            gone.push(format!("{left_out} item(s) left out"));
        }
        if cut {
            gone.push("details cut".to_string());
        }
        let _ = writeln!(
            out,
            "\n(shortened to fit --max-chars: {}. `son show <id>` prints a node in full.)",
            gone.join(", ")
        );
    }
    out
}

fn context_json(ctx: &Context, shape: &ContextShape) -> Value {
    let sections = context_sections(ctx);
    if !shape.budget {
        let mut value = json!({ "collection": node_json(ctx.project) });
        for (name, _, nodes) in sections {
            value[name] = nodes_json(nodes);
        }
        return value;
    }
    if shape.minimal {
        return json!({ "collection": ctx.project.title, "truncated": true });
    }
    let mut value = json!({ "collection": brief_json(ctx.project) });
    let mut cut = false;
    let mut left_out = 0;
    for (name, _, nodes) in sections {
        if shape.bare {
            value[name] = json!(nodes.len());
            left_out += nodes.len();
            continue;
        }
        let shown = &nodes[..nodes.len().min(shape.items)];
        left_out += nodes.len() - shown.len();
        value[name] = Value::Array(
            shown
                .iter()
                .map(|node| {
                    let mut brief = brief_json(node);
                    let body = clip(&node.body, shape.detail);
                    let body_cut = body.trim() != node.body.trim();
                    cut |= body_cut;
                    brief["body"] = json!(body);
                    brief["body_cut"] = json!(body_cut);
                    brief
                })
                .collect(),
        );
    }
    value["truncated"] = json!(cut || left_out > 0);
    value["omitted"] = json!({ "items": left_out, "bodies_cut": cut });
    value
}

/// The text of a context. Without a budget it is the full answer. With
/// one, it is the fullest form that fits; if not even the counts fit, an
/// error.
pub fn context(ctx: &Context, json: bool, max_chars: Option<usize>) -> anyhow::Result<String> {
    let render = |shape: &ContextShape| {
        if json {
            json_text(&context_json(ctx, shape), shape.budget)
        } else {
            context_markdown(ctx, shape)
        }
    };
    let mut shapes = context_shapes();
    match max_chars {
        None => {
            shapes[0].budget = false;
            shapes[0].detail = 600;
            Ok(render(&shapes[0]))
        }
        Some(max) => first_that_fits(shapes.iter().map(render), max),
    }
}
