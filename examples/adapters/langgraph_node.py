"""LangGraph node that runs an .ax program through Boruna as a deterministic,
auditable execution cell and writes the verified result back into graph state.

The node calls the `boruna_run_sealed` MCP tool. Boruna compiles + runs the
`.ax` source, replays it, and returns a verifiable execution record:
`{ result, replay_verified, event_log_sha256, event_log, ... }`. We surface
`result` and `replay_verified` into the graph so downstream nodes (or a human
gate) can branch on whether the run is reproducible.

Status: illustrative. The MCP plumbing (`call_mcp_tool`) is PSEUDO-CODE — wire
it to your MCP client of choice (e.g. the `mcp` Python SDK's stdio client, or
LangChain's MCP adapters). Everything else is real, runnable LangGraph.

Register the server (see README.md in this dir):
    boruna-mcp                       # JSON-RPC over stdio
or, from a checkout:
    cargo run --bin boruna-mcp
"""

from __future__ import annotations

import json
from typing import Any, Optional, TypedDict

from langgraph.graph import END, START, StateGraph


class GraphState(TypedDict, total=False):
    # Input: the .ax program to execute.
    ax_source: str
    # Capability policy — same shape boruna_run/boruna_run_sealed accept:
    # "allow-all", "deny-all", or a policy object. Optional; defaults allow-all.
    policy: Any
    # Outputs written by the boruna node:
    result: Any
    replay_verified: bool
    event_log_sha256: Optional[str]
    error: Optional[str]


# ---------------------------------------------------------------------------
# PSEUDO-CODE: replace with your MCP client call. The contract is:
#   tool "boruna_run_sealed", args {source, policy?, max_steps?}
#   -> returns a JSON *string* (Boruna tools return text content).
# ---------------------------------------------------------------------------
def call_mcp_tool(tool: str, arguments: dict) -> str:  # pragma: no cover
    raise NotImplementedError(
        "Wire this to your MCP client. Example with the `mcp` SDK stdio client:\n"
        "    async with stdio_client(StdioServerParameters(command='boruna-mcp')) as (r, w):\n"
        "        async with ClientSession(r, w) as session:\n"
        "            await session.initialize()\n"
        "            res = await session.call_tool(tool, arguments)\n"
        "            return res.content[0].text\n"
    )


def boruna_sealed_node(state: GraphState) -> GraphState:
    """Run state['ax_source'] through Boruna and record the verified outcome."""
    raw = call_mcp_tool(
        "boruna_run_sealed",
        {
            "source": state["ax_source"],
            "policy": state.get("policy", "allow-all"),
        },
    )
    payload = json.loads(raw)

    if not payload.get("success", False):
        # Domain errors (parse/runtime/capability_denied/invalid_policy) come
        # back as success=false — surface, don't raise, so the graph can route.
        return {
            "result": None,
            "replay_verified": False,
            "event_log_sha256": None,
            "error": f"{payload.get('error_kind')}: {payload.get('message')}",
        }

    return {
        "result": payload["result"],
        "replay_verified": payload["replay_verified"],
        "event_log_sha256": payload.get("event_log_sha256"),
        "error": None,
    }


def build_graph():
    g = StateGraph(GraphState)
    g.add_node("boruna", boruna_sealed_node)
    g.add_edge(START, "boruna")
    g.add_edge("boruna", END)
    return g.compile()


if __name__ == "__main__":  # pragma: no cover
    graph = build_graph()
    out = graph.invoke(
        {
            "ax_source": "fn main() -> Int {\n    2 + 40\n}\n",
            "policy": "allow-all",
        }
    )
    # Only trust the result if the run reproduced deterministically.
    if out["replay_verified"]:
        print("verified result:", out["result"], "seal:", out["event_log_sha256"])
    else:
        print("UNVERIFIED / error:", out.get("error"))
