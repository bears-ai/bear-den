# Human notes

This file is not meant for agents.

Essentially, each Bear is manifest as a Letta Code instance. Den is a control layer that provisions these, their Letta agents, and their capabilities.

Web chat or ACP or API -> Den -> Codepool -> Letta Code -> Letta -> Bifrost (I'm not sure about that Letta step)

Meanwhile, memfs manager brings Letta memory to each code instance

Letta Code instances have an ACP_STRICT_CLIENT_TOOLS mode that denies their access to the local filesystem so they will use ACP and mind only the IDE's filesystem
