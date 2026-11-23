# NoKey Router and Supervised Task Trees

## Router key setup

Default router: `https://nokey-router.ziggyware.deno.net/v1`, model `free:code`. A `ROUTER_API_KEY` is required.

### In the editor

Open **Settings → AI agent configuration** and choose **Save Router API Key…** to enter the raw key. Esc cancels without saving. Authentication failures appear in the transcript.

### From the terminal

```sh
termed --router-login    # hidden input; saves the key and exits
termed --router         # select the router for this launch
termed --router-logout   # remove the saved key and exit
```

`--router-login` imports an already exported `ROUTER_API_KEY` or reads stdin when piped. No key-valued CLI argument exists. An existing nonempty `ROUTER_API_KEY` takes precedence over the saved key.

## Default sub-agent profiles

| Profile | Role |
|---|---|
| **Reviewer** | Leaf for code/safety review. |
| **Test planner** (`tester`) | Leaf for test design and verification planning. |
| **Coordinator** | Delegates to leaves and collects results before returning. |

Leaves cannot gain delegation. Reusing a profile already in the current ancestry is rejected.

## Default limits

| Setting | Default | Allowed values / enforcement |
|---|---|---|
| `max_agent_depth` | 2 | 1–3; root=0, child=1, grandchild=2 |
| `max_delegations` | 16 | 1–64 tasks for the whole root turn |
| `max_concurrent_tasks` | 8 | 1–32 live model sessions |
| `task_timeout_secs` | 300 | 10–1800; root turn deadline |
| `stall_timeout_secs` | 150 | 15–600 without agent activity |
| `max_steps` | 2000 | 1–10000 model rounds |
| `max_output_tokens` | 4096 | 256–16384 completion tokens |
| `generated_tools` | true | Pure interpreted transformations only |
| `toolkit` | true | The 100 extra host tools |
| `toolkit_groups` | all | Empty means every group |
