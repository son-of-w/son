---
name: {name}
description: Manage the backlog of the current project in son, the shared memory. Use it when the user mentions a todo to capture or update, asks to organize, list or complete todos, or calls this skill directly. Todos are kept with the `son` command, not in files.
---

# Todo: a backlog kept in son

This file is the son todo skill, managed by `son init --todo-skill`; changes made here are overwritten. It keeps the backlog of the current collection in `son`, so every session and every agent sees the same todos. Run `son` through the shell. If the project also has a `TODO.md` backlog in files, ask the user which one is the authority; do not keep two.

## Intake

- Use the todo the user gave in the message or in the conversation. If this skill is called with no todo and no other request about the backlog, ask: "What todo would you like to add?" Do not invent one.
- A request to capture a todo is not a request to do the work. Do the work only when the user asks for it.
- Look for the same work first: `son todo backlog`, and `son todo add "<title>" --check`, which lists stored todos that may say the same thing. Look at done todos too: `son todo list --done`. If an open todo covers it, update that one with `son edit`. If a done todo is being opened again, use `son todo reopen <number>`, and say why in its body.
- Collect what a later agent needs: read the code, the docs and the tests that matter. Keep apart what you verified, what the user decided, what is a proposal, and what is not known.
- Ask a question when the answer is needed now. Otherwise write it under "Deferred questions" in the todo, with who answers it and what it blocks. A deferred question must not become an assumption.

## The backlog

- `son todo backlog` prints every open todo of the collection, one line each: priority, number, title. Children are under their parent. The order is by priority (p0 first), then by number, inside each level.
- Each todo has a number that a person can say, such as `#4`, and a son id. Both work in every command: `son show 4`, `son todo done 4.1`. Numbers count for each collection. A number is never used again and never changes, also when the order changes.
- A child of `#4` is `#4.1`; a child of that is `#4.1.1`. Add one with `--parent`: `son todo add "<title>" --parent 4`. Children stay under their parent in every list.
- Priority: `p0` urgent blocker, `p1` high, `p2` normal, `p3` lower, `p4` some day. Use the priority the user gives. With none given, use `p2`, and say in the body that it is a default. Priority is not a way to state the order of work: record a dependency with `son link <todo> depends_on <other todo>`.
- A child can have another priority than its parent. If an urgent child is under a parent of low priority, tell the user; do not change the parent yourself.

## Add a todo

Give the title, the priority, the parent if it has one, and the detail as the body. `--body -` reads the body from stdin:

```
son todo add "<short title>" --priority p2 [--parent <number>] --body -
```

The body is the full record of the todo. Use these sections, and change them to fit the task:

```markdown
## Outcome and scope
What the user wants, why, and the limits.

## Context and implementation notes
What is there now, the files and names that matter, constraints, how to reproduce, a proposed approach.

## Accepted decisions
What the user approved. Keep it apart from your recommendations.

## Dependencies
Other todos by number, and other conditions.

## Acceptance and validation
How to see that it is done, and which checks to run.

## Deferred questions
Each open question, who answers it, and what it blocks. Leave the section out if there is none.
```

- Write enough that another agent can do the work without this conversation. Do not fill a todo with empty headings.
- Say how you learned it: `--evidence user-statement` when the user said it, with `--ref user:"<what they said>"`.
- Never put passwords, tokens or keys in a todo.
- To bring in a backlog that has numbers already, give the number and the status: `son todo add "<title>" --number 4.1 --status done`. Add a parent before its children.

## Change a todo

- `son show <number>` prints the todo in full. Read it before you change it; another agent may have changed it.
- `son edit <number> --title "<title>"`, `--priority p1`, `--body -` (the whole new body from stdin), `--tag a,b`.
- When the user decides something, or answers a deferred question, write it into the body under "Accepted decisions" with the date, and keep the question with its answer.
- `son history <number>` lists every change, and `son undo <number>` reverses the last one.

## Finish a todo

- Check the acceptance criteria, or record that the user confirmed it is done.
- `son todo done <number> --note "<what shows that it is done>"`. The note is added to the body with the date. Name what you checked: the test that passed, the commit, the user's words.
- A todo with open children cannot be done: `son` refuses it. Finish the children first, or ask the user how to arrange them. Do not finish children only to close their parent.
- A done todo is kept, with its number, its body and its history. `son todo list --done` shows it. Do not delete it.
- A todo that will not be done: `son edit <number> --status dropped`, with the reason in the body.
- Never say that a feature is built because its todo is written down.

## Before you stop

Run `son todo backlog` and look at it: each todo you touched is there with the right priority and under the right parent, no todo is there twice, and a finished one is gone from it. Tell the user which numbers you added, changed or finished, and which questions are deferred.
