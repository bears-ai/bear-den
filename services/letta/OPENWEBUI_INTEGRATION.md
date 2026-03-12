# OpenWebUI + Letta Integration Guide

## Current Integration

### Overview

Letta agents are currently connected to OpenWebUI as "models" using functions from the [open-webui-tools](https://github.com/Haervwe/open-webui-tools) repository. This provides a direct integration that allows users to select and interact with Letta agents through OpenWebUI's interface.

### How It Works

1. **Function Installation**: Functions from open-webui-tools are installed in OpenWebUI's Workspace > Functions section
2. **Model Registration**: Letta agents are registered as custom models in OpenWebUI
3. **Direct Communication**: OpenWebUI communicates directly with the Letta API service
4. **Agent Selection**: Users can select Letta agents from OpenWebUI's model dropdown

### Setup Instructions

#### 1. Deploy Letta Service

Ensure Letta is deployed and accessible at `http://bears-letta:8283` (or your configured URL).

#### 2. Install OpenWebUI Tools Function

1. Access your OpenWebUI instance
2. Navigate to **Settings** → **Workspace** → **Functions**
3. Install the Letta integration function from [open-webui-tools](https://github.com/Haervwe/open-webui-tools)
   - Look for functions that connect to external APIs or agent services
   - Alternatively, use the pipe function example in `openwebui_pipe_example.py`
4. Configure the function with:
   - `LETTA_API_URL=http://bears-letta:8283/v1`
   - `LETTA_SERVER_PASS=<your-letta-password>`

#### 3. Register Letta Agents as Models

1. In OpenWebUI, go to **Settings** → **Models**
2. Add a custom model/provider that uses your Letta integration function
3. Configure the model endpoint to use your function
4. Letta agents will now appear in the model selection dropdown

### Configuration

See `openwebui_integration.env.example` for environment variable configuration.

### Session Management

For detailed session management strategies (one agent per user, one per chat, hybrid), see `OPENWEBUI_SESSIONS.md`.

### Limitations of Current Setup

- **No User-Identity Mapping**: OpenWebUI users are not mapped to Letta identities
- **No Access Control**: All agents are available to all users
- **No User-Aware Memory**: Agents don't know which OpenWebUI user they're interacting with
- **Direct API Access**: OpenWebUI communicates directly with Letta without middleware

## Future: Middleware Layer

### Planned Features

A middleware layer will be introduced between OpenWebUI and Letta to provide enhanced functionality:

#### 1. User-Identity Mapping

**Purpose**: Map OpenWebUI users to Letta identities for user-aware interactions.

**Features**:
- Maintain mapping between OpenWebUI user IDs and Letta agent identities
- Support for multiple Letta agents per OpenWebUI user
- Identity context injection into agent requests
- User profile synchronization

**Implementation**:
- Middleware intercepts requests from OpenWebUI to Letta
- Extracts OpenWebUI user ID from request context
- Maps to Letta identity (may create new identity if needed)
- Injects identity context into Letta API calls

#### 2. Agent Access Control

**Purpose**: Restrict which agents ("models") are available to specific users.

**Features**:
- Role-based agent access control
- Per-user agent whitelisting/blacklisting
- Group-based agent permissions
- Dynamic agent availability based on user context

**Implementation**:
- Middleware maintains access control rules (stored in database or config)
- Filters available agents based on user permissions
- Returns only authorized agents in model list
- Enforces access control on agent selection

**Example Configuration**:
```yaml
user_agent_permissions:
  user_alice:
    allowed_agents: ["agent-1", "agent-2"]
    denied_agents: ["agent-3"]
  user_bob:
    allowed_agents: ["agent-2", "agent-3"]
  role_admin:
    allowed_agents: ["*"]  # All agents
```

#### 3. User-Aware Memory Context

**Purpose**: Make agents aware of which OpenWebUI user they're interacting with for personalized memory retrieval.

**Features**:
- User-scoped memory retrieval
- Personal vs. shared memory contexts
- User preference learning
- Context-aware agent responses

**Implementation**:
- Middleware injects user context into memory queries
- Letta agents receive user identity in memory requests
- Memory service filters results based on user context
- Supports both personal (`memories/personal/`) and shared (`memories/shared/`) contexts

**Memory Context Flow**:
```
OpenWebUI User → Middleware → Letta Agent
                         ↓
                   User Context
                         ↓
              Memory Service (with user filter)
                         ↓
              Personal + Shared Memories
```

### Architecture

```
┌─────────────┐
│  OpenWebUI  │
│   :3000     │
└──────┬──────┘
       │
       ↓
┌─────────────────────┐
│   Middleware Layer  │ ← NEW
│                     │
│  - User Mapping     │
│  - Access Control   │
│  - Context Injection│
└──────┬──────────────┘
       │
       ↓
┌─────────────┐
│   Letta     │
│   :8283     │
└─────────────┘
```

### Implementation Plan

#### Phase 1: User-Identity Mapping
- [ ] Create middleware service
- [ ] Implement user-to-identity mapping
- [ ] Add identity context injection
- [ ] Test with single user

#### Phase 2: Access Control
- [ ] Design access control data model
- [ ] Implement permission checking
- [ ] Add agent filtering
- [ ] Create admin interface for permissions

#### Phase 3: User-Aware Memory
- [ ] Integrate user context with memory service
- [ ] Implement personal/shared memory filtering
- [ ] Test memory retrieval with user context
- [ ] Optimize performance

#### Phase 4: Production Hardening
- [ ] Add authentication/authorization
- [ ] Implement caching
- [ ] Add monitoring and logging
- [ ] Performance optimization

### Technology Considerations

**Middleware Options**:
1. **Standalone Service**: Separate service between OpenWebUI and Letta
2. **OpenWebUI Plugin**: Extend OpenWebUI with custom plugin
3. **Letta Middleware**: Add middleware layer within Letta
4. **Reverse Proxy**: Use nginx/traefik with custom logic

**Storage Options**:
- **PostgreSQL**: For user mappings and permissions (already in stack)
- **Redis**: For caching and session data (already in stack)
- **File-based**: For simple configurations (development only)

### Configuration Example

```yaml
middleware:
  user_mapping:
    storage: postgresql
    table: openwebui_user_letta_identity

  access_control:
    storage: postgresql
    table: user_agent_permissions
    default_policy: deny

  memory_context:
    enable_user_scoping: true
    personal_memory_path: memories/personal/{user_id}
    shared_memory_path: memories/shared

  letta:
    api_url: http://bears-letta:8283/v1
    api_key: ${LETTA_SERVER_PASS}
```

## References

- [Open WebUI Documentation](https://docs.openwebui.com)
- [Open WebUI Tools Repository](https://github.com/Haervwe/open-webui-tools)
- [Letta Documentation](https://docs.letta.com)
- [Session Management Guide](OPENWEBUI_SESSIONS.md)
- [Pipe Function Example](openwebui_pipe_example.py)
