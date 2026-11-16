# Issue tracker: Orbit (via MCP)

Issues and specs for this fork live as **tasks in the LegacyWorks Orbit workspace**
(`https://orbit.appster.net`), not on GitHub. GitHub Issues are turned off on `evolv3ai/orbit`;
never file into `coollabsio/orbit` (upstream).

Use the **Orbit MCP server** (`orbit` in Claude Code, endpoint `https://orbit.appster.net/mcp`).
Instance details, tokens and the REST alternative are in the workstation's
`D:\spaces-orbit\integration.md`. The live server is **0.3.0**: its MCP has fewer tools than this
fork's `main` branch, so trust the tools the connection lists, not this repo's `docs/mcp.md`.

## Which project

| Work | Project | Key | Id |
| --- | --- | --- | --- |
| ONLYOFFICE integration in this fork | OnlyOffice Integration | `OO` | `01a1d8d6-4b2c-7329-9b8b-bf7382ead6b8` |
| Anything else in this fork | Ask the user which project | | `list_projects` |

Project home page: the Orbit Doc **OnlyOffice Integration** (spec pointer and status meanings).

## Conventions

- **Create an issue**: `create_task` with `project_id`, `status_id` and `title`; the body goes in
  `description` as Markdown. New issues start in **Needs Triage** unless the user says otherwise.
- **Read an issue**: `get_task` (id or identifier such as `OO-12`), then `list_comments`.
- **List issues**: `list_tasks` with `project_id`, and `query` for text or an identifier. Up to 100 per
  page; follow `next_cursor`. Filter by status client-side using the status ids below.
- **Sub-issue of a parent**: `create_task` with `parent_task_id`; list children with
  `list_tasks(parent_task_id=…)`.
- **Blocking**: Orbit 0.3.0 has no task dependencies. Put `Blocked by: OO-17, OO-18` as the first line of
  the blocked task's description. A task is unblocked when every blocker is **Done** or **Won't Fix**.
- **Comment**: `create_comment` (Markdown).
- **Change status**: `update_task` with `status_id` and the `expected_version` from your latest read. On
  "version changed", read again and retry.
- **Labels**: the 0.3.0 MCP can **read** labels (`list_labels`, `label_ids` on a task) but cannot apply
  them. When a skill says to apply a category label, recommend it in a comment
  (`Category: Bug` or `Category: Enhancement`) and tell the user to set it.
- **Close**: comment with the explanation first, then set the status to **Done** or **Won't Fix**.
  Never delete a task.
- **Statuses and labels cannot be created over MCP.** A person adds them in project settings.

## Claiming and finishing work (agents)

1. **Claim**: assign yourself (current `assignee_ids` plus your `get_me` user id), set **In Progress**,
   comment `AGENT CLAIMED`, then read the task again to confirm no one else claimed it.
2. **Blocked**: ask one specific question in a comment, set **Needs Info**, stop.
3. **Finish**: comment `AGENT DONE` (what changed, where, what you checked, what still needs a person),
   then set **In Review** if a person must approve, publish or decide, otherwise **Done**.
4. **Run breaks**: comment `AGENT FAILED` with the last safe step.

Writes with a personal token appear as that person and send notifications, so say in the text that an
agent wrote it.

## Chat updates

The OO project has a chat channel that agents can post to through an incoming webhook (the MCP has no
chat write). Post **one line** when a task moves to **In Review**, **Done** or **Needs Info**, and when
a run fails (`AGENT FAILED`). Don't post on claim or for routine progress.

- The URL is the secret `ORBIT_CHAT_WEBHOOK_URL_OO` in Infisical project `admin-operator`
  (`b39906ed-a884-4abf-b652-8b964fdfab48`), env `prod`. Read it into a variable; never print, log or
  commit it, and never put it in a task, doc or chat message.
- Body: `{"text": "OO-17 → In Review: versions table added; needs a person to check the migration. <task url>"}`.
  Slack mrkdwn or Markdown, cut at 4,000 characters.
- Success is `200` with the body `ok`. A failed post must not fail the task: note it in the
  `AGENT DONE` comment and move on.
- No customer or tenant personal data, and no secrets, in messages.

```bash
URL=$(infisical secrets get ORBIT_CHAT_WEBHOOK_URL_OO \
  --projectId b39906ed-a884-4abf-b652-8b964fdfab48 --env prod --plain --silent)
curl -sS -m 10 -X POST -H 'Content-Type: application/json' \
  -d '{"text":"OO-17 → In Review: … (agent)"}' "$URL"
unset URL
```

## Status ids: OnlyOffice Integration (`OO`)

| Status | Id |
| --- | --- |
| Needs Triage | `01a1d8d6-4b3f-760c-b661-46b033a4ac43` |
| Needs Info | `01a1e56d-0f2a-745f-9ec6-f89a3168a09f` |
| Ready for Agent | `01a1d8f8-b5f4-77fa-9a3b-26ed45a34b74` |
| Ready for Human | `01a1d8d6-4b40-72f9-a911-4727be1cc333` |
| In Progress | `01a1d8d6-4b40-72f9-a911-4728c9149288` |
| In Review | `01a1d8f9-1178-7723-824c-7afe05ee11aa` |
| Done | `01a1d8d6-4b41-7513-b68c-215d43d4b483` |
| Won't Fix | `01a1d8d6-4b41-7513-b68c-215ef4ea905b` |
| Duplicate | `01a1d8d6-4b41-7513-b68c-215f58a3a39c` |

Status ids are per project. For any other project, call `list_task_statuses` and match by name.

## Pull requests as a triage surface

**PRs as a request surface: no.** _(This fork takes no external PRs; `/triage` reads this flag.)_

## When a skill says "publish to the issue tracker"

Create an Orbit task in the right project (see **Which project**), in **Needs Triage** unless the skill
or the user says the work is already triaged.

## When a skill says "fetch the relevant ticket"

`get_task` plus `list_comments`, as in **Read an issue**.

## Wayfinding operations

Used by `/wayfinder`. The **map** is one parent task; **child** tickets are its sub-issues.

- **Map**: a task titled `[map] <topic>` holding the Notes / Decisions-so-far / Fog body.
- **Child ticket**: `create_task` with `parent_task_id` set to the map. Prefix the title with its type:
  `[research]`, `[prototype]`, `[grilling]` or `[task]` (labels can't be applied over MCP).
- **Blocking**: the `Blocked by:` line, as in Conventions.
- **Frontier query**: `list_tasks(parent_task_id=<map>)`, keep children not Done / Won't Fix / Duplicate,
  drop any with an open blocker or an assignee; first by `number` wins.
- **Claim**: as in **Claiming**, step 1. It is the session's first write.
- **Resolve**: comment the answer, set **Done**, then append a context pointer (gist + identifier) to the
  map's Decisions-so-far with `update_task`.
