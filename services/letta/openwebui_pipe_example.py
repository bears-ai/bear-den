"""
Open WebUI Pipe Function Example for Letta Integration

This is a practical example of how to implement a pipe function that connects
Open WebUI chats to Letta agents with session management.

Usage:
1. Install dependencies: pip install requests redis
2. Set environment variables (see below)
3. Deploy as an Open WebUI pipe function or API endpoint
"""

import os
import json
import hashlib
from typing import Dict, Any, Optional
from datetime import datetime
import requests
import redis

# Configuration
LETTA_API_URL = os.getenv("LETTA_API_URL", "http://bears-letta:8283/v1")
LETTA_API_KEY = os.getenv("LETTA_SERVER_PASS", "")
SESSION_STRATEGY = os.getenv("SESSION_STRATEGY", "user")  # "user", "chat", or "hybrid"
REDIS_URL = os.getenv("REDIS_URL", "redis://bears-redis:6379")

# Initialize Redis client (if using Redis for session storage)
try:
    redis_client = redis.from_url(REDIS_URL) if REDIS_URL else None
except:
    redis_client = None


class LettaSessionManager:
    """Manages session mappings between Open WebUI chats and Letta agents."""

    def __init__(self, storage_backend: str = "redis"):
        self.storage_backend = storage_backend
        if storage_backend == "redis" and redis_client:
            self.storage = redis_client
        else:
            # Fallback to in-memory dict (not persistent)
            self.storage = {}

    def get_chat_agent_mapping(self, chat_id: str) -> Optional[str]:
        """Get the agent ID for a chat."""
        if self.storage_backend == "redis" and redis_client:
            return redis_client.get(f"chat:{chat_id}:agent_id")
        else:
            return self.storage.get(f"chat:{chat_id}:agent_id")

    def set_chat_agent_mapping(self, chat_id: str, user_id: str, agent_id: str, strategy: str):
        """Store the mapping between chat and agent."""
        mapping_data = {
            "agent_id": agent_id,
            "user_id": user_id,
            "strategy": strategy,
            "created_at": datetime.utcnow().isoformat()
        }

        if self.storage_backend == "redis" and redis_client:
            redis_client.set(f"chat:{chat_id}:agent_id", agent_id)
            redis_client.set(f"chat:{chat_id}:mapping", json.dumps(mapping_data))
            if strategy == "user":
                redis_client.set(f"user:{user_id}:default_agent", agent_id)
        else:
            self.storage[f"chat:{chat_id}:agent_id"] = agent_id
            self.storage[f"chat:{chat_id}:mapping"] = mapping_data
            if strategy == "user":
                self.storage[f"user:{user_id}:default_agent"] = agent_id

    def get_user_default_agent(self, user_id: str) -> Optional[str]:
        """Get the default agent for a user (for user-based strategy)."""
        if self.storage_backend == "redis" and redis_client:
            return redis_client.get(f"user:{user_id}:default_agent")
        else:
            return self.storage.get(f"user:{user_id}:default_agent")


class LettaAgentClient:
    """Client for interacting with Letta API."""

    def __init__(self, api_url: str, api_key: str):
        self.api_url = api_url.rstrip("/")
        self.api_key = api_key
        self.headers = {
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json"
        }

    def create_agent(self, name: str, model: str = "gpt-4", **kwargs) -> str:
        """Create a new Letta agent and return its ID."""
        url = f"{self.api_url}/agents"
        payload = {
            "name": name,
            "model": model,
            **kwargs
        }

        try:
            response = requests.post(url, json=payload, headers=self.headers, timeout=10)
            response.raise_for_status()
            agent_data = response.json()
            return agent_data.get("id") or agent_data.get("agent_id")
        except requests.exceptions.RequestException as e:
            raise Exception(f"Failed to create Letta agent: {e}")

    def send_message(self, agent_id: str, message: str) -> str:
        """Send a message to a Letta agent and return the response."""
        url = f"{self.api_url}/agents/{agent_id}/messages"
        payload = {
            "content": message,
            "role": "user"
        }

        try:
            response = requests.post(url, json=payload, headers=self.headers, timeout=30)
            response.raise_for_status()
            result = response.json()

            # Extract response content (adjust based on Letta API response format)
            if isinstance(result, dict):
                return result.get("content") or result.get("message") or result.get("response", "")
            elif isinstance(result, str):
                return result
            else:
                return str(result)
        except requests.exceptions.RequestException as e:
            raise Exception(f"Failed to send message to Letta agent: {e}")

    def get_agent_messages(self, agent_id: str) -> list:
        """Get message history for an agent (if supported by Letta API)."""
        url = f"{self.api_url}/agents/{agent_id}/messages"

        try:
            response = requests.get(url, headers=self.headers, timeout=10)
            response.raise_for_status()
            return response.json()
        except requests.exceptions.RequestException:
            # If endpoint doesn't exist, return empty list
            return []


# Initialize clients
session_manager = LettaSessionManager()
letta_client = LettaAgentClient(LETTA_API_URL, LETTA_API_KEY)


def get_or_create_agent_for_chat(
    chat_id: str,
    user_id: str,
    strategy: str = SESSION_STRATEGY
) -> str:
    """
    Get existing agent for chat or create a new one based on strategy.

    Args:
        chat_id: Open WebUI chat/conversation ID
        user_id: Open WebUI user ID
        strategy: "user" (one agent per user) or "chat" (one agent per chat)

    Returns:
        Letta agent ID
    """
    # Check if mapping already exists
    existing_agent = session_manager.get_chat_agent_mapping(chat_id)
    if existing_agent:
        return existing_agent

    # Create new agent based on strategy
    if strategy == "user":
        # Check if user has a default agent
        user_agent = session_manager.get_user_default_agent(user_id)
        if user_agent:
            agent_id = user_agent
        else:
            # Create new agent for user
            agent_id = letta_client.create_agent(
                name=f"Agent for {user_id}",
                model="gpt-4"
            )
            # Store as user's default agent
            session_manager.set_chat_agent_mapping(chat_id, user_id, agent_id, "user")
    else:  # strategy == "chat"
        # Create new agent for this specific chat
        agent_id = letta_client.create_agent(
            name=f"Agent for chat {chat_id[:8]}",
            model="gpt-4"
        )
        session_manager.set_chat_agent_mapping(chat_id, user_id, agent_id, "chat")

    return agent_id


async def letta_agent_pipe(
    body: Dict[str, Any],
    model: str,
    messages: list,
    **kwargs
) -> Dict[str, Any]:
    """
    Open WebUI pipe function that routes messages to Letta agents.

    This function should be registered as a custom pipe/model in Open WebUI.

    Expected request format (Open WebUI):
    {
        "model": "letta-agent",
        "messages": [
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi there!"},
            {"role": "user", "content": "What's the weather?"}
        ],
        "conversation_id": "chat-123",
        "user_id": "user-456",
        ...
    }
    """
    try:
        # Extract chat and user information
        chat_id = (
            body.get("conversation_id") or
            body.get("chat_id") or
            kwargs.get("conversation_id") or
            body.get("id")
        )

        user_id = (
            body.get("user_id") or
            kwargs.get("user_id") or
            body.get("user", {}).get("id") or
            "anonymous"
        )

        # Generate chat_id if not provided (for new chats)
        if not chat_id:
            # Create a deterministic ID from messages
            messages_hash = hashlib.md5(
                json.dumps(messages, sort_keys=True).encode()
            ).hexdigest()[:8]
            chat_id = f"chat-{messages_hash}"

        # Determine strategy (can be overridden per request)
        strategy = body.get("strategy") or SESSION_STRATEGY

        # Get or create agent for this chat
        agent_id = get_or_create_agent_for_chat(chat_id, user_id, strategy)

        # Extract the last user message
        user_message = None
        for msg in reversed(messages):
            if isinstance(msg, dict) and msg.get("role") == "user":
                content = msg.get("content")
                if isinstance(content, str):
                    user_message = content
                elif isinstance(content, list):
                    # Handle multimodal content
                    text_parts = [part.get("text") for part in content if part.get("type") == "text"]
                    user_message = " ".join(text_parts) if text_parts else None
                break

        if not user_message:
            return {
                "error": {
                    "message": "No user message found in request",
                    "type": "invalid_request"
                }
            }

        # Send message to Letta agent
        try:
            response_content = letta_client.send_message(agent_id, user_message)

            # Return response in Open WebUI format
            return {
                "id": f"chatcmpl-{hashlib.md5(response_content.encode()).hexdigest()[:8]}",
                "object": "chat.completion",
                "created": int(datetime.utcnow().timestamp()),
                "model": model or "letta-agent",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": response_content
                    },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 0,  # Letta handles token counting internally
                    "completion_tokens": 0,
                    "total_tokens": 0
                }
            }
        except Exception as e:
            return {
                "error": {
                    "message": f"Failed to communicate with Letta agent: {str(e)}",
                    "type": "server_error"
                }
            }

    except Exception as e:
        return {
            "error": {
                "message": f"Pipe function error: {str(e)}",
                "type": "internal_error"
            }
        }


# Alternative: Synchronous version (if async not supported)
def letta_agent_pipe_sync(
    body: Dict[str, Any],
    model: str,
    messages: list,
    **kwargs
) -> Dict[str, Any]:
    """Synchronous version of the pipe function."""
    # Same implementation, just not async
    return letta_agent_pipe(body, model, messages, **kwargs)


# Example usage as a standalone API endpoint (Flask/FastAPI)
if __name__ == "__main__":
    # Example with Flask
    try:
        from flask import Flask, request, jsonify
        app = Flask(__name__)

        @app.route("/v1/chat/completions", methods=["POST"])
        async def chat_completions():
            data = request.json
            result = await letta_agent_pipe(
                body=data,
                model=data.get("model", "letta-agent"),
                messages=data.get("messages", []),
                **data
            )
            return jsonify(result)

        @app.route("/health", methods=["GET"])
        def health():
            return jsonify({"status": "healthy"})

        print("Starting Letta pipe function server...")
        print(f"LETTA_API_URL: {LETTA_API_URL}")
        print(f"SESSION_STRATEGY: {SESSION_STRATEGY}")
        app.run(host="0.0.0.0", port=8080, debug=True)

    except ImportError:
        print("Flask not installed. Install with: pip install flask")
        print("\nThis file is meant to be used as a pipe function in Open WebUI.")
        print("See OPENWEBUI_SESSIONS.md for integration instructions.")



