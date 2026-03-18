# Open WebUI + Letta Integration Guide

## Canonical multi-user architecture

For production multi-user deployments, Open WebUI should talk to **Den** (Rust/Axum), which fronts **self-hosted Letta**. See **[DEN_ARCHITECTURE.md](../../DEN_ARCHITECTURE.md)**. Below: **direct** Open WebUI → Letta for dev/single-tenant.

## Current integration (direct)

### Overview

Letta agents are connected to Open WebUI as “models” using functions from the [open-webui-tools](https://github.com/Haervwe/open-webui-tools) repository. That yields direct integration: users select and interact with Letta agents through Open WebUI.

**Knowledge:** Letta **memory** (blocks, conversations) is separate from shared **Cabinet** on **Outline** (via **Den**—[PLAN.md](../../PLAN.md)).

### How it works

1. **Function installation:** Install functions from open-webui-tools in Open WebUI **Workspace → Functions**
2. **Model registration:** Register Letta agents as custom models in Open WebUI
3. **Direct communication:** Open WebUI talks to the Letta API service
4. **Agent selection:** Users pick Letta agents from the model dropdown

### Setup instructions

#### 1. Deploy Letta

Ensure Letta is deployed and reachable at `http://bears-letta:8283` (or your URL).

#### 2. Install Open WebUI tools function

1. Open your Open WebUI instance  
2. **Settings** → **Workspace** → **Functions**  
3. Install the Letta integration from [open-webui-tools](https://github.com/Haervwe/open-webui-tools), or use the pipe example in `openwebui_pipe_example.py`  
4. Configure with:  
   - `LETTA_API_URL=http://bears-letta:8283/v1`  
   - `LETTA_SERVER_PASS=<your-letta-password>`

#### 3. Register Letta agents as models

1. **Settings** → **Models**  
2. Add a custom model/provider that uses your Letta integration function  
3. Configure the endpoint to use that function  
4. Letta agents appear in the model dropdown

### Configuration

See `openwebui_integration.env.example` for environment variables.

### Session management

For session strategies (one agent per user, per chat, hybrid), see [OPENWEBUI_SESSIONS.md](OPENWEBUI_SESSIONS.md).

### Limitations of direct setup

- **No user–identity mapping:** Open WebUI users are not mapped to Letta identities  
- **No access control:** All agents are visible to all users  
- **No user-aware memory:** Agents are not tied to a specific Open WebUI user  
- **Direct API access:** Traffic goes straight to Letta  

For multi-user production, use **Den** (Axum): [DEN_ARCHITECTURE.md](../../DEN_ARCHITECTURE.md) — self-hosted Letta only.

## References

- **[DEN_ARCHITECTURE.md](../../DEN_ARCHITECTURE.md)** — Den (Axum) + self-hosted Letta  
- [Open WebUI documentation](https://docs.openwebui.com)  
- [open-webui-tools](https://github.com/Haervwe/open-webui-tools)  
- [Letta documentation](https://docs.letta.com)  
- [Session management](OPENWEBUI_SESSIONS.md)  
- [Pipe function example](openwebui_pipe_example.py)  
