# Open WebUI + Letta Session Management Guide

## Overview

This guide explains how to implement session management in Letta and map those sessions to chats in Open WebUI. Since Letta uses stateful agents with persistent memory and Open WebUI uses chat sessions, we need to establish a mapping strategy.

## Architecture

### Letta's Agent Model

- **Stateful Agents**: Each Letta agent maintains a single, continuous message history
- **Persistent Memory**: All interactions contribute to the agent's long-term memory
- **No Traditional Sessions**: Letta doesn't use ephemeral sessions; agents are persistent

### Open WebUI's Chat Model

- **Chat Sessions**: Each chat has its own persistent history
- **User Management**: Open WebUI tracks users and their conversations
- **Session Isolation**: Each chat session is independent

## Mapping Strategies

### Strategy 1: One Agent Per User (Recommended for Personalization)

**Approach**: Create one Letta agent per Open WebUI user. All chats from that user share the same agent's memory.

**Pros**:
- Agent learns user preferences across all conversations
- Consistent personality and context across chats
- Better long-term memory and personalization

**Cons**:
- All chats share the same context (may mix topics)
- Less isolation between different conversation threads

**Use Case**: Personal assistant that should remember you across all interactions

### Strategy 2: One Agent Per Chat (Recommended for Isolation)

**Approach**: Create one Letta agent per Open WebUI chat session. Each chat has its own isolated agent.

**Pros**:
- Complete isolation between conversations
- Each chat maintains its own context
- Easier to manage and debug individual conversations

**Cons**:
- No cross-chat memory or learning
- More agents to manage
- Less personalized experience

**Use Case**: Project-specific chats, topic-specific conversations, or when you want clean separation

### Strategy 3: Hybrid (Recommended for Flexibility)

**Approach**: Use a combination - one agent per user by default, but allow creating new agents for specific chats when needed.

**Pros**:
- Flexibility to choose isolation level
- Can create project-specific agents
- Balance between personalization and isolation

**Cons**:
- More complex mapping logic
- Need to track agent purpose/scope

## Implementation

### Step 1: Session Mapping Storage

You need to store the mapping between Open WebUI chats and Letta agents. Options:

#### Option A: Redis (Fast, Ephemeral)

```python
# Store mapping: chat_id -> agent_id
redis.set(f"chat:{chat_id}:agent_id", agent_id)
redis.set(f"user:{user_id}:default_agent", agent_id)  # For user-based mapping
```

#### Option B: PostgreSQL (Persistent, Queryable)

```sql
CREATE TABLE chat_agent_mapping (
    id SERIAL PRIMARY KEY,
    chat_id VARCHAR(255) UNIQUE NOT NULL,
    user_id VARCHAR(255) NOT NULL,
    agent_id VARCHAR(255) NOT NULL,
    strategy VARCHAR(50) NOT NULL,  -- 'user' or 'chat'
    created_at TIMESTAMP DEFAULT NOW(),
    updated_at TIMESTAMP DEFAULT NOW()
);

CREATE INDEX idx_chat_id ON chat_agent_mapping(chat_id);
CREATE INDEX idx_user_id ON chat_agent_mapping(user_id);
```

#### Option C: File-based (Simple, for Development)

```json
{
  "mappings": {
    "chat-123": {
      "agent_id": "agent-abc",
      "user_id": "user-1",
      "strategy": "chat",
      "created_at": "2025-01-15T10:00:00Z"
    }
  },
  "user_agents": {
    "user-1": "agent-xyz"
  }
}
```

### Step 2: Letta Agent Management Functions

Create helper functions to manage Letta agents:

```python
import requests
import os

LETTA_API_URL = os.getenv("LETTA_API_URL", "http://bears-letta:8283/v1")
LETTA_API_KEY = os.getenv("LETTA_SERVER_PASS")

def create_letta_agent(name: str, model: str = "gpt-4", user_id: str = None) -> str:
    """Create a new Letta agent and return its ID."""
    url = f"{LETTA_API_URL}/agents"
    headers = {
        "Authorization": f"Bearer {LETTA_API_KEY}",
        "Content-Type": "application/json"
    }
    payload = {
        "name": name,
        "model": model,
        # Add any other agent configuration
    }
    response = requests.post(url, json=payload, headers=headers)
    response.raise_for_status()
    agent_data = response.json()
    return agent_data["id"]

def get_or_create_agent_for_chat(chat_id: str, user_id: str, strategy: str = "user") -> str:
    """Get existing agent for chat or create a new one."""
    # Check if mapping exists
    mapping = get_chat_agent_mapping(chat_id)
    if mapping:
        return mapping["agent_id"]

    # Create new agent based on strategy
    if strategy == "user":
        # Check if user has a default agent
        user_agent = get_user_default_agent(user_id)
        if user_agent:
            # Use existing user agent
            agent_id = user_agent
        else:
            # Create new agent for user
            agent_id = create_letta_agent(
                name=f"Agent for {user_id}",
                user_id=user_id
            )
            set_user_default_agent(user_id, agent_id)
    else:  # strategy == "chat"
        # Create new agent for this chat
        agent_id = create_letta_agent(
            name=f"Agent for chat {chat_id}",
            user_id=user_id
        )

    # Store mapping
    set_chat_agent_mapping(chat_id, user_id, agent_id, strategy)
    return agent_id

def send_message_to_agent(agent_id: str, message: str) -> str:
    """Send a message to a Letta agent and get response."""
    url = f"{LETTA_API_URL}/agents/{agent_id}/messages"
    headers = {
        "Authorization": f"Bearer {LETTA_API_KEY}",
        "Content-Type": "application/json"
    }
    payload = {
        "content": message
    }
    response = requests.post(url, json=payload, headers=headers)
    response.raise_for_status()
    return response.json()["content"]
```

### Step 3: Open WebUI Pipe Function

Create a pipe function in Open WebUI that routes messages to Letta agents:

```python
# openwebui_pipe_function.py

import os
import json
from typing import Dict, Any

# Import your agent management functions
from letta_agent_manager import get_or_create_agent_for_chat, send_message_to_agent

async def letta_agent_pipe(
    body: Dict[str, Any],
    model: str,
    messages: list,
    **kwargs
) -> Dict[str, Any]:
    """
    Open WebUI pipe function that routes messages to Letta agents.

    This function:
    1. Extracts chat_id and user_id from the request
    2. Gets or creates a Letta agent for the chat
    3. Sends the message to the agent
    4. Returns the agent's response
    """

    # Extract chat and user information
    # Open WebUI typically includes this in the request context
    chat_id = body.get("conversation_id") or body.get("chat_id") or kwargs.get("conversation_id")
    user_id = body.get("user_id") or kwargs.get("user_id")

    if not chat_id:
        # Generate a chat_id if not provided (for new chats)
        chat_id = f"chat-{hash(json.dumps(messages))}"

    if not user_id:
        user_id = "anonymous"

    # Determine strategy (can be passed as parameter or use default)
    strategy = body.get("strategy", "user")  # or "chat" for isolation

    # Get or create agent for this chat
    agent_id = get_or_create_agent_for_chat(chat_id, user_id, strategy)

    # Extract the last user message
    user_message = None
    for msg in reversed(messages):
        if msg.get("role") == "user":
            user_message = msg.get("content")
            break

    if not user_message:
        return {
            "error": "No user message found"
        }

    # Send message to Letta agent
    try:
        response_content = send_message_to_agent(agent_id, user_message)

        # Return response in Open WebUI format
        return {
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": response_content
                },
                "finish_reason": "stop"
            }],
            "model": model,
            "usage": {
                "prompt_tokens": 0,  # Letta handles token counting
                "completion_tokens": 0,
                "total_tokens": 0
            }
        }
    except Exception as e:
        return {
            "error": str(e)
        }
```

### Step 4: Register Pipe Function in Open WebUI

In Open WebUI, register your pipe function as a custom model:

1. **Via Open WebUI UI**:
   - Go to Settings → Models
   - Add Custom Model
   - Set model name (e.g., `letta-agent`)
   - Set API endpoint to your pipe function endpoint

2. **Via Open WebUI API** (if supported):

```python
# Register the pipe function
pipe_config = {
    "name": "letta-agent",
    "type": "pipe",
    "function": letta_agent_pipe,
    "description": "Routes messages to Letta agents with session management"
}
```

### Step 5: Enhanced Session Management

Add session metadata tracking:

```python
def create_chat_session(chat_id: str, user_id: str, title: str = None, strategy: str = "user"):
    """Create a new chat session with agent mapping."""
    agent_id = get_or_create_agent_for_chat(chat_id, user_id, strategy)

    session_data = {
        "chat_id": chat_id,
        "user_id": user_id,
        "agent_id": agent_id,
        "strategy": strategy,
        "title": title,
        "created_at": datetime.utcnow().isoformat(),
        "message_count": 0
    }

    # Store session metadata
    store_session_metadata(session_data)

    return session_data

def update_session_metadata(chat_id: str, **updates):
    """Update session metadata (e.g., message count, last activity)."""
    # Update session tracking
    pass

def get_session_history(chat_id: str) -> list:
    """Get conversation history for a chat session."""
    mapping = get_chat_agent_mapping(chat_id)
    if not mapping:
        return []

    agent_id = mapping["agent_id"]
    # Fetch message history from Letta agent
    # (Letta API may provide this)
    return get_agent_messages(agent_id)
```

## Advanced Features

### Session Archiving

When a chat is archived in Open WebUI, you can optionally archive the Letta agent:

```python
def archive_chat_session(chat_id: str, archive_agent: bool = False):
    """Archive a chat session and optionally its agent."""
    mapping = get_chat_agent_mapping(chat_id)
    if not mapping:
        return

    # Mark session as archived
    update_session_metadata(chat_id, archived=True, archived_at=datetime.utcnow())

    if archive_agent:
        # Optionally archive or delete the agent
        # (Letta may support agent archiving)
        archive_letta_agent(mapping["agent_id"])
```

### Session Summarization

Periodically summarize long sessions:

```python
def summarize_session(chat_id: str):
    """Generate a summary of the session for memory promotion."""
    mapping = get_chat_agent_mapping(chat_id)
    if not mapping:
        return

    agent_id = mapping["agent_id"]
    history = get_agent_messages(agent_id)

    # Use Letta agent to summarize
    summary = send_message_to_agent(
        agent_id,
        f"Please summarize this conversation: {json.dumps(history)}"
    )

    # Store summary in episodic memory (history/)
    store_session_summary(chat_id, summary)
```

### Multi-User Session Sharing

For shared chats (e.g., team channels):

```python
def create_shared_agent(chat_id: str, user_ids: list, name: str = None):
    """Create an agent shared by multiple users."""
    agent_id = create_letta_agent(
        name=name or f"Shared agent for {chat_id}",
        user_id=None  # No single user owner
    )

    # Store mapping with multiple users
    set_chat_agent_mapping(chat_id, None, agent_id, "shared")
    set_shared_chat_users(chat_id, user_ids)

    return agent_id
```

## Integration with BEARS Memory System

Since you're using the BEARS stack, you can integrate session data with your memory system:

```python
def save_session_to_history(chat_id: str, user_id: str):
    """Save session to BEARS history/ directory."""
    mapping = get_chat_agent_mapping(chat_id)
    if not mapping:
        return

    agent_id = mapping["agent_id"]
    history = get_agent_messages(agent_id)

    # Format as episodic memory
    session_data = {
        "session_id": chat_id,
        "timestamp": datetime.utcnow().isoformat(),
        "user": user_id,
        "agent_id": agent_id,
        "interactions": history,
        "summary": None  # Can be generated later
    }

    # Save to history/ directory via knowledgebase API
    knowledgebase_url = os.getenv("KNOWLEDGEBASE_URL", "http://bears-knowledgebase:8080")
    requests.post(
        f"{knowledgebase_url}/history/sessions",
        json=session_data
    )
```

## Configuration

### Environment Variables

Add to your Letta service configuration:

```bash
# Letta API Configuration
LETTA_API_URL=http://bears-letta:8283/v1
LETTA_SERVER_PASS=<your-password>

# Session Management Strategy
SESSION_STRATEGY=user  # or "chat" or "hybrid"

# Storage Backend
SESSION_STORAGE=redis  # or "postgres" or "file"
REDIS_URL=redis://bears-redis:6379
# or
POSTGRES_URL=postgresql://user:pass@host:5432/db

# Knowledgebase Integration
KNOWLEDGEBASE_URL=http://bears-knowledgebase:8080
```

## Testing

### Test Session Creation

```python
# Test creating a session
chat_id = "test-chat-1"
user_id = "test-user"
agent_id = get_or_create_agent_for_chat(chat_id, user_id, "user")
print(f"Created agent {agent_id} for chat {chat_id}")

# Test sending a message
response = send_message_to_agent(agent_id, "Hello, this is a test message")
print(f"Agent response: {response}")

# Test retrieving same agent for same chat
agent_id_2 = get_or_create_agent_for_chat(chat_id, user_id, "user")
assert agent_id == agent_id_2, "Should return same agent"
```

### Test Chat Isolation

```python
# Test that different chats get different agents (if using "chat" strategy)
chat1_id = "chat-1"
chat2_id = "chat-2"
user_id = "test-user"

agent1 = get_or_create_agent_for_chat(chat1_id, user_id, "chat")
agent2 = get_or_create_agent_for_chat(chat2_id, user_id, "chat")

assert agent1 != agent2, "Different chats should have different agents"
```

## Troubleshooting

### Agent Not Found

- Check Letta API connectivity
- Verify `LETTA_API_URL` and `LETTA_SERVER_PASS` are correct
- Check Letta service logs

### Session Mapping Lost

- If using Redis, check Redis persistence
- If using PostgreSQL, verify database connectivity
- Consider adding backup/recovery for mappings

### Context Mixing

- If using "user" strategy, chats share context (by design)
- Switch to "chat" strategy for isolation
- Or implement hybrid strategy with explicit chat scoping

## Next Steps

1. **Choose a strategy** (user/chat/hybrid) based on your use case
2. **Implement storage** for session mappings (Redis/PostgreSQL/file)
3. **Create pipe function** in Open WebUI
4. **Test integration** with sample chats
5. **Integrate with BEARS memory** system for history tracking
6. **Add session management UI** (optional) for viewing/managing sessions

## References

- [Letta Documentation](https://docs.letta.com)
- [Open WebUI Documentation](https://docs.openwebui.com)
- [Letta API Reference](https://docs.letta.com/api-reference)
- [Open WebUI Function Calling](https://docs.openwebui.com/features/function-calling)



