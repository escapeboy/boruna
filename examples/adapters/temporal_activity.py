"""Temporal Activity wrapping Boruna as a deterministic, auditable execution cell.

Why an Activity and not Workflow code: Temporal *workflow* code must itself be
deterministic and may not do I/O, so calling an external process/MCP server
belongs in an **Activity**. The nice property here is that Boruna's own
execution is deterministic and replay-verified, so the Activity returns a
`replay_verified` flag plus a `event_log_sha256` seal the workflow can persist
or assert on.

The Activity calls the `boruna_run_sealed` MCP tool (or, equivalently, the
`boruna run` CLI). It returns a small dataclass the workflow can store.

Status: illustrative. `call_mcp_tool` is PSEUDO-CODE — wire it to your MCP
client. The Temporal activity/worker scaffolding is real (temporalio SDK).
"""

from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Optional

from temporalio import activity


@dataclass
class SealedRunRequest:
    ax_source: str
    policy: Any = "allow-all"  # "allow-all" | "deny-all" | policy object
    max_steps: int = 10_000_000


@dataclass
class SealedRunResult:
    success: bool
    result: Any
    replay_verified: bool
    event_log_sha256: Optional[str]
    error: Optional[str]


# ---------------------------------------------------------------------------
# PSEUDO-CODE: replace with your MCP client call, OR shell out to the CLI:
#   subprocess.run(["boruna", "run", path, "--policy", "allow-all"], ...)
# The MCP tool returns richer data (replay_verified + seal), so it is preferred.
# ---------------------------------------------------------------------------
def call_mcp_tool(tool: str, arguments: dict) -> str:  # pragma: no cover
    raise NotImplementedError("Wire this to your MCP client — see README.md")


@activity.defn
async def run_boruna_sealed(req: SealedRunRequest) -> SealedRunResult:
    raw = call_mcp_tool(
        "boruna_run_sealed",
        {"source": req.ax_source, "policy": req.policy, "max_steps": req.max_steps},
    )
    payload = json.loads(raw)

    if not payload.get("success", False):
        return SealedRunResult(
            success=False,
            result=None,
            replay_verified=False,
            event_log_sha256=None,
            error=f"{payload.get('error_kind')}: {payload.get('message')}",
        )

    return SealedRunResult(
        success=True,
        result=payload["result"],
        replay_verified=payload["replay_verified"],
        event_log_sha256=payload.get("event_log_sha256"),
        error=None,
    )


# --- Example workflow using the activity (illustrative) --------------------
# from datetime import timedelta
# from temporalio import workflow
#
# @workflow.defn
# class DeterministicCellWorkflow:
#     @workflow.run
#     async def run(self, ax_source: str) -> SealedRunResult:
#         res = await workflow.execute_activity(
#             run_boruna_sealed,
#             SealedRunRequest(ax_source=ax_source),
#             start_to_close_timeout=timedelta(seconds=30),
#         )
#         # Fail the workflow if the cell did not reproduce deterministically.
#         if res.success and not res.replay_verified:
#             raise workflow.ApplicationError("boruna run was not replay-verified")
#         return res
