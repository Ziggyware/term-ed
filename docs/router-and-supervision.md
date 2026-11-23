# NoKey Router and Supervised Task Trees

## Router key setup

The built-in Rust agent defaults to `https://nokey-router.ziggyware.deno.net/v1` and the `free:code` model. termed requires a `ROUTER_API_KEY`, even though the router's upstream uses free models.

### In the editor

1. Open **Settings → AI agent configuration** (or the agent panel's **⚙**).
2. Choose **Save Router API Key…**, enter the raw key in the masked field, then press Enter. Esc cancels without saving.
3. Send a task. Authentication and provider failures appear in the transcript; saving a key does not validate it or start a model request.

If selecting the router fails because user settings cannot be saved, the key is still saved locally. Fix the settings and choose **Use NoKey Router**. That action can also switch an existing configuration to the router without re-entering the key. It selects `builtin` + `router` + `free:code`.

### From the terminal

```sh
termed --router-login    # hidden input; saves the key and exits
termed --router         # select the router for this launch
termed --router-logout   # remove the saved key and exit
```

`--router-login` imports an already exported `ROUTER_API_KEY`, or reads stdin when piped. There is deliberately no key-valued command-line argument. An existing nonempty `ROUTER_API_KEY` takes precedence over the saved key at runtime; the editor warns if that override is active when saving a key.

## Default sub-agent profiles

New configurations include three editable read-only profiles:

| Profile | Role |
|---|---|
| **Reviewer** | Leaf for code/safety review. |
| **Test planner** (`tester`) | Leaf for test design and verification planning. |
| **Coordinator** | May delegate focused investigations to the leaves and must collect their results before returning. |

Both the root and a coordinator must permit delegation; a leaf cannot gain delegation by inventing a tool call. Reusing a profile already in the current ancestry is rejected, preventing coordinator→coordinator cycles.

## Default limits

| Setting | Default | Allowed values / enforcement |
|---|---|---|
| `max_agent_depth` | 2 | 1–3; root=0, child=1, grandchild=2; every ancestor may lower the cap |
| `max_delegations` | 16 | 1–64 tasks for the whole root turn, including descendants and queued requests |
| `max_concurrent_tasks` | 8 | 1–32 live model sessions; siblings run in parallel |
| `task_timeout_secs` | 300 | 10–1800; root turn deadline |
| `stall_timeout_secs` | 150 | 15–600 without agent activity |
| `max_steps` | 2000 | 1–10000 model rounds |
| `max_output_tokens` | 4096 | 256–16384 completion tokens |
| `generated_tools` | true | Pure interpreted transformations only |
| `toolkit` | true | The 100 extra host tools |
| `toolkit_groups` | all | Empty means every group; intersection with parent's groups |
