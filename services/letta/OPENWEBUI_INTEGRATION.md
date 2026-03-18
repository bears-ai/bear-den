# OpenWebUI + Letta Integration Guide

## Canonical Multi-User Architecture

For production multi-user deployments, OpenWebUI should talk to **Den** (Rust/Axum), which fronts **self-hosted Letta**. See **[MULTIUSER_PROXY_ARCHITECTURE.md](../../MULTIUSER_PROXY_ARCHITECTURE.md)**. Below: **direct** OpenWebUI → Letta for dev/single-tenant.

## Current Integration (Direct)

### Overview

Letta agents are currently connected to OpenWebUI as "models" using functions from the [open-webui-tools](https://github.com/Haervwe/open-webui-tools) repository. This provides a direct integration that allows users to select and interact with Letta agents through OpenWebUI's interface.

**Knowledge:** Letta **memory** (blocks, conversations) is separate from shared **Cabinet** on **Outline** (via **Den**—[PLAN.md](../../PLAN.md)).

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

### Limitations of Direct Setup

- **No User-Identity Mapping**: OpenWebUI users are not mapped to Letta identities
- **No Access Control**: All agents are available to all users
- **No User-Aware Memory**: Agents don't know which OpenWebUI user they're interacting with
- **Direct API Access**: OpenWebUI communicates directly with Letta

For multi-user production, use **Den** (Axum): [MULTIUSER_PROXY_ARCHITECTURE.md](../../MULTIUSER_PROXY_ARCHITECTURE.md) — self-hosted Letta only.

## References

- **[MULTIUSER_PROXY_ARCHITECTURE.md](../../MULTIUSER_PROXY_ARCHITECTURE.md)** – Den (Axum) + self-hosted Letta
- [Open WebUI Documentation](https://docs.openwebui.com)
- [Open WebUI Tools Repository](https://github.com/Haervwe/open-webui-tools)
- [Letta Documentation](https://docs.letta.com)
- [Session Management Guide](OPENWEBUI_SESSIONS.md)
- [Pipe Function Example](openwebui_pipe_example.py)
