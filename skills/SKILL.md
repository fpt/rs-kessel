---
name: desk-activity
description: "Use when the user asks what they are working on / doing, to log or track their activity, to update their task board, or to create a task from what's on screen. Looks at the screen and connects it to the user's GitHub Projects tasks."
---
Connect what's on the user's screen to their GitHub Projects board: figure out what
they're working on, match it to a task, and (with their OK) update the board.

Requires the `list_windows` / `capture_screen` / `apply_ocr` tools and the `github_*`
tools. If the github tools aren't available, the board isn't configured — just report
the on-screen activity and say the board isn't connected.

## Workflow

1. **See the desktop.** Call `list_windows` to get the open windows (system UI and
   incognito windows are already filtered out — never try to work around that).

2. **Read the relevant windows.** Pick the 1–3 windows that show real work (editor,
   browser, terminal, docs — not chat or music). For each, `capture_screen(window_id)`
   to look, and `apply_ocr` when you need exact text (file names, URLs, error
   messages, ticket numbers). Don't capture every window — focus on the foreground work.

3. **Get the board.** Call `github_list_tasks` to load the user's assigned tasks
   (note each task's `item_id` — the other github tools need it).

4. **Match.** Decide what the user is doing and which task it best fits, using the
   window titles, OCR text, repo names, and issue numbers. If nothing matches, say so.

5. **Report — briefly.** One or two spoken sentences: what they're doing and the
   matched task. e.g. "You're editing the auth handler in api-server — that looks like
   #497, refactor login." Never list raw tool output or window dumps.

## Updating the board

Only change the board when the user asks or clearly agrees. These tools each prompt for
confirmation on the terminal, so propose the action in words first, then call the tool:

- **Mark progress** — `github_set_status(item_id, status)` (e.g. "In Progress").
- **Log activity** — `github_log_activity(item_id, text, context?)` posts a short
  activity comment on the issue. Summarize what they did; put file/URL details in `context`.
  (Drafts have no issue — promote first.)
- **Capture new work** — if there's no matching task, offer `github_create_draft(title, body)`.
- **Promote a draft** — `github_promote_draft(item_id)` turns a draft into a real assigned issue.

## Notes

- Privacy: incognito/system windows are excluded by `list_windows`; don't try to read them.
- The output is usually spoken (TTS) — keep it short, natural, and free of tool/field names.
- If `list_windows` is empty or a capture fails, say what you could and don't guess.
