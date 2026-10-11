# Plugin runtime

Portable manifest-and-skill packages and their MCP contributions are implemented by [`aidex-extensions`](../extensions/README.md), including a versioned local first-party marketplace catalog. Packages require explicit trust and scope grants to become agent capabilities. No third-party executable plugin code is loaded into the daemon; MCP processes use the owned sandbox lifecycle.
