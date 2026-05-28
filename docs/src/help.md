# Help

Claude Code Rust has two built-in help paths:

```text
/help
/docs <topic>
```

`/help` opens the fullscreen Help tab. Use it when you want navigable in-app help without adding a message to the chat transcript.

`/docs` renders live help into the chat. Use it when you want the current commands, shortcuts, modes, models, or subagents copied into the conversation as reference material.

## Fullscreen Help

The fullscreen Help tab has three sections:

| Section | Shows |
| --- | --- |
| Shortcuts | Keyboard shortcuts for the current app state and focused UI context. |
| Commands | App-owned slash commands plus slash commands advertised by the active SDK session. |
| Subagents | Subagents advertised by the active SDK session, including model labels when provided. |

Use `Left` and `Right` to switch Help sections. Use `Up` and `Down` to move through the visible rows in the active section.

The Shortcuts section is state-sensitive. It changes depending on whether focus is in chat input, autocomplete, an inline permission prompt, an inline question, or a blocked state such as connecting or command-pending.

The Commands and Subagents sections are also session-sensitive. While the app is connecting, they show loading rows. If the active session does not advertise SDK commands or subagents, the Help tab shows an empty-state row instead of inventing unavailable entries.

## In-Chat Docs

| Command | Shows |
| --- | --- |
| `/docs mode` | Current and available session modes. |
| `/docs models` | Models advertised by the active session. |
| `/docs shortcuts` | Keyboard shortcuts for the current app state. |
| `/docs commands` | App-owned and SDK-advertised slash commands. |
| `/docs agents` | Subagents advertised by the active session. |

`/docs` covers two topics that are not sections in the fullscreen Help tab: `mode` and `models`. Those topics are based on the active session's advertised modes and models.

The `/docs` output is based on the running app state. It can differ from this manual when the active session advertises different models, modes, commands, or agents.
