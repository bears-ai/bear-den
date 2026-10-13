# Den

## Bear

### Chat surfaces

#### Slack chat

#### Web chat

- chat via API

#### Web chat with sandbox

- chat via API
- sandbox workspace browser

#### ACP chat (Zed)

- CLI protocol via armature over BearWire

### Bear admin

#### Dashboard

#### Bear identity
- bear name
- ID slug

##### Prompts
- identity prompt
- stance prompts

#### Configuration

##### Models
- default (tiered)
- stance overrides

#### Memory
- metrics
- search
- browse

##### Reflections
- log

#### Skills

#### Context review

#### Capabilities
##### Tools
##### Connections
##### Network (resources)

#### Cabinet awareness (planned)
- Bear-level Cabinet capability/enablement settings
- authorized access, review and retention summaries
- links to the distinct Cabinet surface for live page details, content operations and page access/review policy; no duplicate inline Cabinet browser/editor
- lifecycle blockers stay in Bear admin; readable Cabinet remediation opens on Cabinet without granting access to private evidence

#### Docket
#### People
#### Activity
##### LLM usage

#### Advanced
- details
- import
- export

## Cabinet surface (planned)

Target, not current deployment: [Cabinet Phase 5](docs/roadmap/CABINET_IMPLEMENTATION_PLAN.md#phase-5--distinct-cabinet-surface-and-external-mcp).

- distinct public hostname and dedicated Cabinet-only container, still using Den's shared implementation, identity/policy and canonical Postgres database
- distinct Cabinet landing page and primary navigation: browse/search and page tree, reviews, your uploads and saved copies as authorized
- page content/details, history, create/edit, organization, page membership/policy, review, lifecycle, sources and attachment/file workflows
- explicit authorized links back to Den/Bear/Job context, not the full Bear/chat/Work/operator navigation rail
- authenticated MCP access for external agent runtimes over the same Cabinet facade; not a second knowledge or permission store

Docket retains Job→Mission reference and evidence controls, linking to Cabinet for the live page. Bear-specific Domains and private memory remain Bear-owned, not Cabinet Missions.
