"""ADK agent definitions for the Aura memory system."""

from google.adk.agents import Agent

from config import INGEST_MODEL, CONSOLIDATE_MODEL, QUERY_MODEL
from tools import (
    store_memory,
    read_recent_memories,
    read_unconsolidated_memories,
    store_consolidation,
    mark_consolidated,
    read_consolidation_history,
    read_all_memories,
    search_memories,
    get_memory_stats,
)

ingest_agent = Agent(
    name="ingest_agent",
    model=INGEST_MODEL,
    instruction=(
        "You are a memory extraction agent for Aura, a macOS desktop voice assistant. "
        "Analyze conversation transcripts and extract structured memories.\n\n"
        "Extract facts about the user's preferences, habits, entities they work with, "
        "tasks they perform, and useful context. Be selective — only store information "
        "worth remembering across sessions.\n\n"
        "If the session was trivial (just a greeting, test, or very short), store nothing.\n\n"
        "Before storing, use read_recent_memories to check for duplicates. "
        "Do not store a memory if a very similar one already exists.\n\n"
        "Categories: preference, habit, entity, task, context.\n"
        "Importance: 0.0 (trivial) to 1.0 (critical). Most facts are 0.4-0.7.\n\n"
        "After extracting memories, respond with a JSON summary of what you stored:\n"
        '{"summary": "brief session summary", "facts": [{"category": "...", "content": "...", '
        '"entities": [...], "importance": 0.5}]}'
    ),
    tools=[store_memory, read_recent_memories],
)

consolidate_agent = Agent(
    name="consolidate_agent",
    model=CONSOLIDATE_MODEL,
    instruction=(
        "You are a memory consolidation agent. Review unconsolidated memories, "
        "find connections between them, and generate insights.\n\n"
        "Like the human brain during sleep — compress, connect, and synthesize. "
        "Look for patterns across sessions.\n\n"
        "Steps:\n"
        "1. Use read_unconsolidated_memories to get unprocessed memories\n"
        "2. Use read_consolidation_history to see what insights already exist\n"
        "3. Find connections: shared entities, related topics, behavioral patterns\n"
        "4. Use store_consolidation to save your findings\n"
        "5. Use mark_consolidated to mark processed memories\n\n"
        "If there are fewer than 2 unconsolidated memories, respond that there's "
        "nothing to consolidate yet."
    ),
    tools=[
        read_unconsolidated_memories,
        read_consolidation_history,
        store_consolidation,
        mark_consolidated,
    ],
)

query_agent = Agent(
    name="query_agent",
    model=QUERY_MODEL,
    instruction=(
        "You are a memory retrieval agent for Aura, a macOS desktop voice assistant. "
        "Given the user's current context (screen content, time, recent activity), "
        "find the most relevant memories and synthesize a brief context summary.\n\n"
        "Be concise — this will be injected into a real-time voice conversation. "
        "Return 2-4 sentences max.\n\n"
        "Steps:\n"
        "1. Use search_memories with keywords from the context\n"
        "2. Use read_all_memories for recent history\n"
        "3. Use read_consolidation_history for cross-session insights\n"
        "4. Synthesize the most relevant information into a brief summary\n\n"
        "Focus on actionable context: what was the user doing, what do they care about, "
        "what patterns are relevant right now."
    ),
    tools=[read_all_memories, read_consolidation_history, search_memories, get_memory_stats],
)
